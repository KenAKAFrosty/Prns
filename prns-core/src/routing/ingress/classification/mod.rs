use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::{InboundPacket, InterfaceId, InterfaceKind};
use crate::routing::announce::Announce;
use crate::routing::dedup::PacketHash;
use crate::routing::NextHop;
use crate::wire::{
    wire_hop_count_is_valid, IfacFlag, PacketType, WireAddress, WireContext, WirePacketHeader,
    BROADCAST_MTU,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DataPacket<'a> {
    pub header: WirePacketHeader,
    pub payload: &'a mut [u8],
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Ingress<'a> {
    Announce {
        packet_hash: PacketHash,
        identity_hash: IdentityHash,
        announce: Announce<'a>,
        payload: &'a [u8],
        header: WirePacketHeader,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
        next_hop: NextHop,
        is_path_response: bool,
    },

    Data {
        packet_hash: PacketHash,
        data: DataPacket<'a>,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    LinkRequest {
        packet_hash: PacketHash,
        payload: &'a [u8],
        header: WirePacketHeader,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    Proof {
        packet_hash: PacketHash,
        payload: &'a [u8],
        address: WireAddress,
        context: WireContext,
        received_hops: u8,
        source_interface: InterfaceId,
        arrived_at: InstantMillis,
    },

    Malformed,
    IfacRefused,
}

#[derive(Debug)]
pub struct ClassifiedInboundPacket<'a> {
    source_interface: InterfaceId,
    ingress: Ingress<'a>,
}

impl<'a> ClassifiedInboundPacket<'a> {
    #[must_use]
    pub fn classify(packet: InboundPacket<'a>) -> Self {
        let source_interface = packet.source_interface;
        Self {
            source_interface,
            ingress: Ingress::classify(packet),
        }
    }

    #[must_use]
    pub fn packet_hash(&self) -> Option<PacketHash> {
        self.ingress.packet_hash()
    }

    #[must_use]
    pub fn proof(&self) -> Option<(WireAddress, &[u8])> {
        match &self.ingress {
            Ingress::Proof {
                address, payload, ..
            } => Some((*address, *payload)),
            Ingress::Announce { .. }
            | Ingress::Data { .. }
            | Ingress::LinkRequest { .. }
            | Ingress::Malformed
            | Ingress::IfacRefused => None,
        }
    }

    #[must_use]
    pub fn is_malformed(&self) -> bool {
        matches!(self.ingress, Ingress::Malformed)
    }

    pub(crate) fn into_parts(self) -> (InterfaceId, Ingress<'a>) {
        (self.source_interface, self.ingress)
    }
}

fn local_adjusted_hops(received_hops: u8, source: InterfaceId) -> u8 {
    if source.kind() == Some(InterfaceKind::LocalClient) {
        received_hops.saturating_sub(1)
    } else {
        received_hops
    }
}

impl<'a> Ingress<'a> {
    #[must_use]
    pub fn packet_hash(&self) -> Option<PacketHash> {
        match self {
            Self::Announce { packet_hash, .. }
            | Self::Data { packet_hash, .. }
            | Self::LinkRequest { packet_hash, .. }
            | Self::Proof { packet_hash, .. } => Some(*packet_hash),
            Self::Malformed | Self::IfacRefused => None,
        }
    }

    pub fn classify(packet: InboundPacket<'a>) -> Self {
        let InboundPacket {
            arrived_at,
            source_interface,
            bytes,
        } = packet;
        let (header, payload_offset) = match WirePacketHeader::parse(bytes) {
            Ok((header, payload)) => (header, bytes.len() - payload.len()),
            Err(_) => return Self::Malformed,
        };
        if !wire_hop_count_is_valid(header.hops) {
            return Self::Malformed;
        }
        if header.ifac_flag == IfacFlag::Authenticated {
            return Self::IfacRefused;
        }
        let (_, payload) = bytes.split_at_mut(payload_offset);
        let packet_hash = PacketHash::of_fields(
            header.destination_type,
            header.packet_type,
            &header.address,
            header.context,
            payload,
        );

        let received_hops = local_adjusted_hops(header.hops.saturating_add(1), source_interface);

        match header.packet_type {
            PacketType::Announce => {
                let payload: &'a [u8] = payload;
                let Ok((announce, identity_hash)) =
                    Announce::from_wire_unverified_with_identity(&header, payload)
                else {
                    return Self::Malformed;
                };

                debug_assert!(
                    {
                        let mut scratch = [0u8; BROADCAST_MTU];
                        announce
                            .to_wire(&mut scratch)
                            .map(|n| &scratch[..n] == payload)
                            .unwrap_or(false)
                    },
                    "Announce::to_wire∘from_wire must round-trip, else a rebroadcast re-emits a signature-broken packet"
                );

                Self::Announce {
                    packet_hash,
                    identity_hash,
                    announce,
                    payload,
                    header,
                    received_hops,
                    source_interface,
                    arrived_at,
                    next_hop: header.transport_id.map_or(NextHop::Direct, NextHop::Via),
                    is_path_response: header.context == WireContext::PathResponse,
                }
            }
            PacketType::Data => Self::Data {
                packet_hash,
                data: DataPacket { header, payload },
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::LinkRequest => Self::LinkRequest {
                packet_hash,
                payload,
                header,
                received_hops,
                source_interface,
                arrived_at,
            },
            PacketType::Proof => Self::Proof {
                packet_hash,
                payload,
                address: header.address,
                context: header.context,
                received_hops,
                source_interface,
                arrived_at,
            },
        }
    }
}

#[cfg(test)]
mod tests;
