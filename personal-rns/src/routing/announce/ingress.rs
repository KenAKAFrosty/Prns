use crate::engine::egress::PATH_REQUEST_DESTINATION;
use crate::engine::EngineState;
use crate::engine::InstantMillis;
use crate::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use crate::routing::announce::announce_rate::AnnounceRateVerdict;
use crate::routing::announce::defaults::{
    jitter_offset_for, JitterSeed, DEFAULT_REBROADCAST_JITTER_WINDOW_MS,
};
use crate::routing::announce::held_cache::HeldAnnounces;
use crate::routing::announce::schedule::RebroadcastQueue;
use crate::routing::announce::Announce;
use crate::routing::announce::{AnnounceAcceptanceDecision, AnnounceAcceptanceInput};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::delivery::{
    Delivery, PlainDelivery, SingleDelivery, PLAIN_DATA_MAX_RECEIVED_HOPS,
};
use crate::routing::path_requests::seen::{PathRequestIdBytes, PathRequestNovelty};
use crate::routing::proof::{ProofIngest, ProofOwed};
use crate::routing::reverse_routes::{ReverseRouteEntry, DEFAULT_REVERSE_ROUTE_TIMEOUT_MS};
use crate::routing::storage::EngineStorage;
use crate::routing::upstream_app_destinations::{ProofStrategy, UpstreamAppDestinationKind};
use crate::routing::NextHop;
use crate::routing::{DropCause, UpsertRouteOutcome};
use crate::wire::{ContextFlag, IfacFlag, PropagationType};
use crate::wire::{
    DestinationHash, DestinationType, PacketType, TransportId, WireContext, WireError,
    WirePacketHeader, MTU, TRUNCATED_HASH_BYTE_LEN,
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
        is_path_response: bool,
    },

    Data {
        data: DataPacket<'a>,
        header: WirePacketHeader,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    LinkRequest,

    Proof {
        payload: &'a [u8],
        destination: DestinationHash,
        received_hops: u8,
        source_interface: InterfaceId,
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
                    is_path_response: header.context == WireContext::PathResponse,
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
                header,
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::LinkRequest => Self::LinkRequest,
            PacketType::Proof => Self::Proof {
                payload,
                destination: header.destination,
                received_hops,
                source_interface,
                arrived_at,
            },
        }
    }
}

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
    /// A path response is learned but never re-flooded — the answer is for the
    /// requester, not the network (RNS Transport.py:1884).
    TerminalPathResponse,
    /// The route is learned, but the destination is announcing faster than the
    /// receiving interface's rate target allows, so its rebroadcast is suppressed
    /// for a penalty window (RNS Transport.py:1835-1887).
    RateBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestPacketOutcome<'p> {
    Announce(AnnounceIngest),
    Delivery {
        delivery: Delivery<'p>,
        maybe_owed_proof: Option<ProofOwed>,
    },
    Proof(ProofIngest),
    Forward(PacketToForward<'p>),
    /// A path request arrived for one of our own destinations — the runtime
    /// owes a path-response announce for it.
    AnswerPathRequest {
        destination: DestinationHash,
    },
    /// A path request arrived for a destination we relay but do not own — the
    /// runtime owes a re-emission of the announce we cached for it.
    AnswerPathRequestFromCache {
        destination: DestinationHash,
    },
    Ignored,
}

/// A packet in transport, re-framed and owed to another interface — RNS 1.3.1
/// Transport.py:1556-1580 (data riding the path table onward) and :2254 (a
/// proof riding the reverse table home).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketToForward<'p> {
    pub header: WirePacketHeader,
    pub payload: &'p [u8],
    pub fire_on: InterfaceId,
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

impl<S: EngineStorage> EngineState<S> {
    #[must_use]
    pub fn ingest_packet<'p>(
        &mut self,
        packet: InboundPacket<'p>,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
    ) -> IngestPacketOutcome<'p> {
        self.ingested_packet_count = self.ingested_packet_count.saturating_add(1);

        match Ingress::classify(packet) {
            Ingress::Announce {
                announce,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
                is_path_response,
            } => IngestPacketOutcome::Announce(self.ingest_announce(
                announce,
                received_hops,
                source_interface,
                arrived_at,
                next_hop,
                is_path_response,
                jitter,
                interfaces,
            )),

            Ingress::Data {
                data,
                header,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                if data.destination == PATH_REQUEST_DESTINATION
                    && data.destination_type == DestinationType::Plain
                {
                    return self.ingest_path_request(&data);
                }
                let in_transport_through_us = self.transport_id.is_some()
                    && header.transport_id == self.transport_id
                    && self
                        .upstream_app_destinations
                        .lookup(&data.destination, data.destination_type)
                        .is_none();
                if in_transport_through_us {
                    return match self.maybe_forward(
                        header,
                        data.payload,
                        received_hops,
                        source_interface,
                        arrived_at,
                    ) {
                        Some(forward) => IngestPacketOutcome::Forward(forward),
                        None => IngestPacketOutcome::Ignored,
                    };
                }
                match self.maybe_upstream_delivery(
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
                }
            }

            Ingress::Proof {
                payload,
                destination,
                received_hops,
                source_interface,
                arrived_at,
            } => {
                if let Some(reverse) = self.reverse_routes.take(&destination, arrived_at) {
                    // The proof must arrive back over the interface we forwarded
                    // toward; anything else is dropped (Transport.py:2256).
                    if reverse.outbound_interface != source_interface {
                        return IngestPacketOutcome::Ignored;
                    }
                    return IngestPacketOutcome::Forward(PacketToForward {
                        header: WirePacketHeader {
                            ifac_flag: IfacFlag::Open,
                            context_flag: ContextFlag::Unset,
                            propagation: PropagationType::Broadcast,
                            destination_type: DestinationType::Single,
                            packet_type: PacketType::Proof,
                            hops: received_hops,
                            transport_id: None,
                            destination,
                            context: WireContext::None,
                        },
                        payload,
                        fire_on: reverse.received_interface,
                    });
                }
                IngestPacketOutcome::Proof(self.ingest_proof(payload, arrived_at))
            }

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

    /// RNS 1.3.1 `Transport.path_request_handler`: the payload opens with the
    /// requested destination hash (an optional requester transport id and tag
    /// follow, both deferred to the relay/forward work). We answer only requests
    /// for a destination of our own; relaying a request we can't answer is later
    /// work.
    fn ingest_path_request<'p>(&mut self, data: &DataPacket<'_>) -> IngestPacketOutcome<'p> {
        // The leaf form: the requested destination then a random id (the
        // optional requester transport id is deferred to a later slice).
        let (Some(destination), Some(id)) = (
            data.payload
                .get(..TRUNCATED_HASH_BYTE_LEN)
                .and_then(DestinationHash::from_slice),
            data.payload
                .get(TRUNCATED_HASH_BYTE_LEN..TRUNCATED_HASH_BYTE_LEN * 2)
                .and_then(|bytes| PathRequestIdBytes::try_from(bytes).ok()),
        ) else {
            return IngestPacketOutcome::Ignored;
        };

        // A request we have already seen (same destination and id) is a loop or
        // a re-arrival — drop it before answering or forwarding again.
        if self.seen_path_requests.observe(destination, id) == PathRequestNovelty::Duplicate {
            return IngestPacketOutcome::Ignored;
        }

        // We answer a path request we can satisfy — for one of our own
        // destinations, or, as a transport node, for a route we hold. We never
        // *forward* an unknown onward: that is opt-in recursive discovery
        // (RNS `DISCOVER_PATHS_FOR`), gated off and built later.
        if self
            .upstream_app_destinations
            .lookup(&destination, DestinationType::Single)
            .is_some()
        {
            IngestPacketOutcome::AnswerPathRequest { destination }
        } else if self.transport_id.is_some()
            && self
                .routing_table
                .retained_announce_for(&destination)
                .is_some()
        {
            IngestPacketOutcome::AnswerPathRequestFromCache { destination }
        } else {
            IngestPacketOutcome::Ignored
        }
    }

    /// RNS 1.3.1 Transport.py:1556-1580: a transport-addressed packet rides the
    /// path table onward. It's re-addressed at the next relay while more than one
    /// hop remains, stripped back to a plain broadcast for the final hop. It also
    /// leaves a reverse-table row so its proof can ride home.
    fn maybe_forward<'p>(
        &mut self,
        header: WirePacketHeader,
        payload: &'p mut [u8],
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    ) -> Option<PacketToForward<'p>> {
        if header.destination_type != DestinationType::Single
            || header.packet_type != PacketType::Data
        {
            return None;
        }
        let route = self
            .routing_table
            .forwarding_route_for(&header.destination)?;

        let packet_hash = PacketHash::of_data_fields(
            header.destination_type,
            &header.destination,
            header.context,
            payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return None,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }

        let forwarded_header = if route.hops == 1 {
            WirePacketHeader {
                ifac_flag: IfacFlag::Open,
                context_flag: ContextFlag::Unset,
                propagation: PropagationType::Broadcast,
                destination_type: header.destination_type,
                packet_type: header.packet_type,
                hops: received_hops,
                transport_id: None,
                destination: header.destination,
                context: header.context,
            }
        } else {
            let NextHop::Via(next) = route.next_hop else {
                return None;
            };
            WirePacketHeader {
                hops: received_hops,
                transport_id: Some(next),
                ..header
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

        Some(PacketToForward {
            header: forwarded_header,
            payload,
            fire_on: route.receiving_interface,
        })
    }

    /// Off (false) when the interface sets no target, which is the reference default (RNS Transport.py:1836).
    fn announce_rate_blocks_rebroadcast(
        &mut self,
        source_interface: InterfaceId,
        destination: DestinationHash,
        now: InstantMillis,
        interfaces: &[InterfaceConfig],
    ) -> bool {
        let Some(limit) = interfaces
            .iter()
            .find(|descriptor| descriptor.id == source_interface)
            .and_then(|descriptor| descriptor.announce_rate_limit)
        else {
            return false;
        };
        self.announce_rates.observe(destination, now, limit) == AnnounceRateVerdict::Blocked
    }

    #[allow(clippy::too_many_arguments)]
    fn ingest_announce(
        &mut self,
        announce: Announce<'_>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        is_path_response: bool,
        jitter: JitterSeed,
        interfaces: &[InterfaceConfig],
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
                let rebroadcast = if is_path_response {
                    RebroadcastDecision::TerminalPathResponse
                } else if self.transport_id.is_none() {
                    RebroadcastDecision::NotATransportNode
                } else if !interfaces
                    .iter()
                    .any(|descriptor| descriptor.capabilities.allows_transport())
                {
                    RebroadcastDecision::NoTransportInterfaces
                } else if self.announce_rate_blocks_rebroadcast(
                    source_interface,
                    announce.destination,
                    arrived_at,
                    interfaces,
                ) {
                    RebroadcastDecision::RateBlocked
                } else {
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
            header: _,
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
    fn a_path_request_for_a_local_destination_owes_an_answer() {
        let mut state = personal_node_announcer();
        let local = state.self_announced_destinations()[0];

        let mut buf = [0u8; MTU];
        let n =
            crate::engine::write_path_request_wire_packet(local, &[0x55; 16], &mut buf).unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            state.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::AnswerPathRequest { destination: local },
        );
    }

    #[test]
    fn a_leaf_ignores_a_path_request_for_a_stranger() {
        // A non-transport node with no route to the destination has nothing to
        // answer and nothing to forward.
        let mut leaf: EngineState<Cap> = EngineState::<Cap>::default();
        let mut buf = [0u8; MTU];
        let n = crate::engine::write_path_request_wire_packet(
            DestinationHash::new([0x44; 16]),
            &[0x55; 16],
            &mut buf,
        )
        .unwrap();
        let mut wire = buf[..n].to_vec();
        assert_eq!(
            leaf.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn write_path_response_announce_emits_a_path_response_a_peer_learns_as_a_route() {
        use crate::engine::PathResponseWriteOutcome;
        use crate::routing::announce::Announce;

        // B answers for its own destination with a PATH_RESPONSE announce.
        let mut b = personal_node_announcer();
        let local = b.self_announced_destinations()[0];
        let mut buf = [0u8; MTU];
        let PathResponseWriteOutcome::Written { wire_len } = b.write_path_response_announce(
            &local,
            InstantMillis(500),
            TEST_SELF_ANNOUNCE_ENTROPY,
            &mut buf,
        ) else {
            panic!("a local destination is answerable");
        };

        let (header, payload) = WirePacketHeader::parse(&buf[..wire_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.context, WireContext::PathResponse);
        assert_eq!(header.destination, local);
        assert_eq!(
            Announce::from_wire(&header, payload).unwrap().destination,
            local
        );

        // A fresh peer accepts it as an ordinary announce — a learned route.
        let mut a: EngineState<Cap> = EngineState::<Cap>::default();
        let mut wire = buf[..wire_len].to_vec();
        assert!(matches!(
            a.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_200),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        assert_eq!(a.route_count(), 1);
    }

    #[test]
    fn a_path_response_for_a_destination_we_do_not_hold_is_refused() {
        use crate::engine::PathResponseWriteOutcome;
        let mut b = personal_node_announcer();
        let mut buf = [0u8; MTU];
        assert!(matches!(
            b.write_path_response_announce(
                &DestinationHash::new([0x44; 16]),
                InstantMillis(500),
                TEST_SELF_ANNOUNCE_ENTROPY,
                &mut buf,
            ),
            PathResponseWriteOutcome::NotLocal,
        ));
    }

    fn relay_holding_a_cached_route() -> (EngineState<Cap>, DestinationHash) {
        let cached =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());
        let mut relay = transporting_node();
        let mut announce = hx(RAW_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xB2),
                    bytes: &mut announce,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
        (relay, cached)
    }

    fn path_request_wire(destination: DestinationHash) -> std::vec::Vec<u8> {
        let mut buf = [0u8; MTU];
        let n = crate::engine::write_path_request_wire_packet(destination, &[0x55; 16], &mut buf)
            .unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn a_transport_node_answers_a_path_request_from_its_cache() {
        let (mut relay, cached) = relay_holding_a_cached_route();
        let mut wire = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::AnswerPathRequestFromCache {
                destination: cached
            },
        );
    }

    #[test]
    fn a_leaf_with_a_route_but_no_transport_role_does_not_answer_from_cache() {
        let cached =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());
        let mut leaf: EngineState<Cap> = EngineState::<Cap>::default();
        let mut announce = hx(RAW_ANNOUNCE);
        let _ = leaf.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: iface(0xB2),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut wire = path_request_wire(cached);
        assert_eq!(
            leaf.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "without a transport role a node never answers from cache, even holding the route",
        );
    }

    #[test]
    fn a_cached_path_response_re_emits_the_retained_announce_stamped_for_transport() {
        use crate::engine::CachedPathResponseOutcome;
        use crate::wire::PropagationType;

        let (relay, cached) = relay_holding_a_cached_route();
        let mut buf = [0u8; MTU];
        let CachedPathResponseOutcome::Written { wire_len } =
            relay.write_cached_path_response(&cached, &mut buf)
        else {
            panic!("the relay holds a route to re-emit");
        };

        let (header, _) = WirePacketHeader::parse(&buf[..wire_len]).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.propagation, PropagationType::Transport);
        assert_eq!(header.transport_id, Some(TEST_TRANSPORT_ID));
        assert_eq!(header.destination, cached);
        assert_eq!(
            header.context,
            WireContext::None,
            "a relay's answer is a plain transport retransmission, not a PATH_RESPONSE",
        );

        assert!(matches!(
            relay.write_cached_path_response(&DestinationHash::new([0x44; 16]), &mut buf),
            CachedPathResponseOutcome::Unavailable,
        ));
    }

    #[test]
    fn a_path_response_is_learned_as_a_route_but_never_rebroadcast() {
        let mut relay = transporting_node();
        let mut response = hx(RAW_ANNOUNCE);
        // Tag the announce as a path response by flipping its context byte.
        response[HEADER_MIN_LEN - 1] = WireContext::PathResponse.to_byte();

        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut response,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                destination: DestinationHash::new(
                    hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
                ),
                hops: 1,
                rebroadcast: RebroadcastDecision::TerminalPathResponse,
            })),
        );
        assert_eq!(relay.route_count(), 1, "the path response is learned");
        assert_eq!(
            relay.pending_announce_rebroadcast_count(),
            0,
            "a path response is never re-flooded",
        );
    }

    #[test]
    fn the_same_announce_without_the_path_response_tag_is_scheduled() {
        let mut relay = transporting_node();
        let mut announce = hx(RAW_ANNOUNCE);
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(500),
                    source_interface: iface(0xA1),
                    bytes: &mut announce,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(relay.pending_announce_rebroadcast_count(), 1);
    }

    #[test]
    fn a_destination_announcing_faster_than_the_interface_target_is_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;
        use crate::routing::announce::SelfAnnounceEntropy;

        // A peer mints two distinct announces for its own destination.
        let mut announcer = personal_node_announcer();
        let destination = announcer.self_announced_destinations()[0];
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };
        let mut buf_a = [0u8; MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                SelfAnnounceEntropy::new([0x11; SelfAnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                SelfAnnounceEntropy::new([0x22; SelfAnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        // The receiving interface caps a destination to one announce per 10s.
        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        // First sighting: learned and scheduled to rebroadcast.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(10_000),
                    source_interface: source,
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        // A second announce 1s later — far under the 10s target — is learned but
        // its rebroadcast is suppressed.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(11_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::RateBlocked,
                ..
            })),
        ));
        assert_eq!(relay.route_count(), 1, "the route is still learned");
        assert_eq!(
            relay.pending_announce_rebroadcast_count(),
            1,
            "only the first announce was scheduled to rebroadcast",
        );
    }

    #[test]
    fn a_destination_within_the_interface_target_is_not_rate_blocked() {
        use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
        use crate::interfaces::AnnounceRateLimit;
        use crate::routing::announce::SelfAnnounceEntropy;

        let mut announcer = personal_node_announcer();
        let destination = announcer.self_announced_destinations()[0];
        let command = AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Scheduled,
        };
        let mut buf_a = [0u8; MTU];
        let first_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(1_000),
                SelfAnnounceEntropy::new([0x11; SelfAnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_a,
            )
            .written_len();
        let mut first = buf_a[..first_len].to_vec();
        let mut buf_b = [0u8; MTU];
        let second_len = announcer
            .write_commanded_announce(
                &command,
                InstantMillis(2_000),
                SelfAnnounceEntropy::new([0x22; SelfAnnounceEntropy::LEN]),
                TEST_RATCHET_ENTROPY,
                &mut buf_b,
            )
            .written_len();
        let mut second = buf_b[..second_len].to_vec();

        let source = iface(0xB2);
        let rate_limited = [InterfaceConfig {
            announce_rate_limit: Some(AnnounceRateLimit {
                target_ms: 10_000,
                grace: 0,
                penalty_ms: 60_000,
            }),
            ..routable_descriptor(source)
        }];

        let mut relay = transporting_node();
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(10_000),
                source_interface: source,
                bytes: &mut first,
            },
            TEST_ENTROPY,
            &rate_limited,
        );
        // A second announce a full target window later stays under the limit and
        // is scheduled like any other.
        assert!(matches!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(25_000),
                    source_interface: source,
                    bytes: &mut second,
                },
                TEST_ENTROPY,
                &rate_limited,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(AcceptedAnnounce {
                rebroadcast: RebroadcastDecision::Scheduled,
                ..
            })),
        ));
        assert_eq!(
            relay.pending_announce_rebroadcast_count(),
            1,
            "one pending per destination — the second schedule replaces the first",
        );
    }

    #[test]
    fn a_transport_node_with_no_route_does_not_forward_the_request() {
        // Forwarding an unknown onward is opt-in recursive discovery (off by
        // default), so a relay that holds no route simply ignores the request.
        let mut relay = transporting_node();
        let mut wire = path_request_wire(DestinationHash::new([0x44; 16]));
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut wire,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
        );
    }

    #[test]
    fn a_duplicate_path_request_is_not_answered_twice() {
        // Dedup is always on: a relay answers once from cache, and a re-arrival
        // of the same (destination, id) is dropped.
        let (mut relay, cached) = relay_holding_a_cached_route();

        let mut first = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: iface(0xA1),
                    bytes: &mut first,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::AnswerPathRequestFromCache {
                destination: cached
            },
        );

        let mut echo = path_request_wire(cached);
        assert_eq!(
            relay.ingest_packet(
                InboundPacket {
                    arrived_at: InstantMillis(1_100),
                    source_interface: iface(0xB2),
                    bytes: &mut echo,
                },
                TEST_ENTROPY,
                &transporting_view(),
            ),
            IngestPacketOutcome::Ignored,
            "the same (destination, id) is a duplicate, not answered again",
        );
    }

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
                TEST_SELF_ANNOUNCE_ENTROPY,
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
                TEST_SELF_ANNOUNCE_ENTROPY,
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
    fn a_final_hop_forward_strips_the_transport_header_back_to_the_direct_wire() {
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 16]),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 16]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a one-hop route forwards, got {out:?}");
        };
        assert_eq!(forward.fire_on, InterfaceId::new([0xB2; 16]));
        let mut wire = [0u8; MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = hx(RAW_SEALED_TO_RATCHET);
        expected[1] = 1;
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "the final hop strips transport framing: the destination hears the direct wire, one hop further",
        );

        let mut replay = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let again = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xA1; 16]),
                bytes: &mut replay,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            again,
            IngestPacketOutcome::Ignored,
            "a relay forwards each packet exactly once",
        );
    }

    #[test]
    fn a_mid_path_forward_swaps_the_transport_id_to_the_next_relay() {
        use crate::wire::PropagationType;

        let next_relay = TransportId::new([0xBB; 16]);
        let mut relay = transporting_node();

        let raw = hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE);
        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();
        let relayed_header = WirePacketHeader {
            transport_id: Some(next_relay),
            propagation: PropagationType::Transport,
            hops: 1,
            ..header
        };
        let mut relayed = [0u8; MTU];
        let header_len = relayed_header.write(&mut relayed).unwrap();
        relayed[header_len..header_len + payload.len()].copy_from_slice(payload);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 16]),
                bytes: &mut relayed[..header_len + payload.len()],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 16]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );

        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("a transport-addressed packet with a multi-hop route forwards, got {out:?}");
        };
        let mut wire = [0u8; MTU];
        let n = forward.to_wire(&mut wire).unwrap();
        let mut expected = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        expected[1] = 1;
        expected[2..18].copy_from_slice(next_relay.as_bytes());
        assert_eq!(
            &wire[..n],
            expected.as_slice(),
            "mid-path the only bytes that change are the hop count and the next relay's id",
        );
    }

    #[test]
    fn a_proof_rides_the_reverse_route_home_exactly_once() {
        let mut relay = transporting_node();
        let mut announce = hx(RATCHETED_SELF_ANNOUNCE_RNS_WIRE);
        let _ = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(500),
                source_interface: InterfaceId::new([0xB2; 16]),
                bytes: &mut announce,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let mut in_transport = hx(RAW_SEALED_TO_RATCHET_VIA_TRANSPORT);
        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xA1; 16]),
                bytes: &mut in_transport,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let IngestPacketOutcome::Forward(forward) = out else {
            panic!("the data leg must forward first");
        };
        let proof_destination = PacketHash::of_data_fields(
            forward.header.destination_type,
            &forward.header.destination,
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
            destination: proof_destination,
            context: WireContext::None,
        };
        let mut proof_wire = [0u8; MTU];
        let header_len = proof_header.write(&mut proof_wire).unwrap();
        proof_wire[header_len..header_len + 64].fill(0xAB);
        let proof_len = header_len + 64;

        let mut wrong_lane = proof_wire;
        let mut right_lane = proof_wire;

        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface: InterfaceId::new([0xB2; 16]),
                bytes: &mut right_lane[..proof_len],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        let IngestPacketOutcome::Forward(returned) = out else {
            panic!("the proof must ride the reverse route, got {out:?}");
        };
        assert_eq!(
            returned.fire_on,
            InterfaceId::new([0xA1; 16]),
            "the proof leaves on the interface the data packet arrived from",
        );
        let mut wire = [0u8; MTU];
        let n = returned.to_wire(&mut wire).unwrap();
        let mut expected = std::vec::Vec::new();
        expected.extend_from_slice(&proof_wire[..proof_len]);
        expected[1] = 1;
        assert_eq!(&wire[..n], expected.as_slice());

        let out = relay.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(3_000),
                source_interface: InterfaceId::new([0xB2; 16]),
                bytes: &mut wrong_lane[..proof_len],
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            out,
            IngestPacketOutcome::Proof(crate::engine::ProofIngest::Ignored),
            "reverse rows pop on use: the second copy finds no path home",
        );
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
        let mut state = EngineState::<
            FixedInline<4, 64, 8, 4, 512, 64, 8, 8, 8, 128, 8, 8, 8, 8, 16>,
        >::default();

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
