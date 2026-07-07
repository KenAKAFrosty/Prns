pub const TRUNCATED_HASH_BYTE_LEN: usize = 16;
pub const ANNOUNCE_PUBLIC_KEY_BYTE_LEN: usize = 64;
pub const DOTTED_NAME_HASH_BYTE_LEN: usize = 10;
pub const RATCHET_BYTE_LEN: usize = 32;
pub const SIGNATURE_BYTE_LEN: usize = 64;

/// RNS 1.3.5 `Reticulum.MTU`: the maximum byte size of one packet on the broadcast plane (announces, path requests, un-linked Singles), which peers must agree on.
/// A wire-protocol invariant, permanently 500 even on fat pipes; the per-link MTU a LINKREQUEST negotiates is separate.
/// Everything that allocates per-packet scratch (announce reassembly, payload buffers) bounds against it.
pub const BROADCAST_MTU: usize = 500;
/// RNS 1.3.5 `Transport.PATHFINDER_M`: packets beyond this hop count are outside reach.
/// A wire-protocol invariant, not a sizing knob.
pub const MAX_HOP_COUNT: u8 = 128;
/// The type-1 (direct, no transport id) header:
/// `flags, hops, destination, context`
pub const HEADER_MIN_LEN: usize = 2 + TRUNCATED_HASH_BYTE_LEN + 1;
/// RNS 1.3.5 `Reticulum.HEADER_MAXSIZE`. The type-2 (transport-routed) header:
/// `flags, hops, transport id, destination, context`
/// Outbound payload budgets reserve this even when emitting type-1, because a relay re-emits the packet with the transport id added.
pub const HEADER_MAX_LEN: usize = 2 + TRUNCATED_HASH_BYTE_LEN * 2 + 1;
/// RNS 1.3.5 `Reticulum.IFAC_MIN_SIZE`: the smallest per-interface access-code overhead a packet may gain; reserved in every payload budget like the transport header.
pub const IFAC_MIN_LEN: usize = 1;
/// RNS 1.3.5 `Reticulum.MDU`: the most payload bytes one packet may carry once the worst-case header and minimum IFAC are reserved.
pub const BROADCAST_MDU: usize = BROADCAST_MTU - HEADER_MAX_LEN - IFAC_MIN_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    BufferTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContextFlag {
    Unset = 0b0,
    Set = 0b1,
}

impl ContextFlag {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Unset,
            _ => Self::Set,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PropagationType {
    Broadcast = 0b0,
    Transport = 0b1,
}

impl PropagationType {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Broadcast,
            _ => Self::Transport,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DestinationType {
    Single = 0b00,
    Group = 0b01,
    Plain = 0b10,
    Link = 0b11,
}

impl DestinationType {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Self::Single,
            0b01 => Self::Group,
            0b10 => Self::Plain,
            _ => Self::Link,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Data = 0b00,
    Announce = 0b01,
    LinkRequest = 0b10,
    Proof = 0b11,
}

impl PacketType {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Self::Data,
            0b01 => Self::Announce,
            0b10 => Self::LinkRequest,
            _ => Self::Proof,
        }
    }
}

/// Every writer packs Open because that is what RNS 1.3.5 does at construction.
/// IFAC is an interface-boundary transform, where `Transport.transmit` masks the raw packet and flips this flag per interface key, and `Transport.inbound` unmasks or drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IfacFlag {
    Open = 0b0,
    Authenticated = 0b1,
}

impl IfacFlag {
    const fn from_bits(bits: u8) -> Self {
        match bits & 0b1 {
            0b0 => Self::Open,
            _ => Self::Authenticated,
        }
    }
}

/// The trailing context byte: a sub-type tag the engine routes on.
/// Exhaustive over the values RNS defines, plus `Unknown(u8)` so an unrecognised byte round-trips unchanged (RNS preserves unknown context bytes).
/// Decoding only ever yields `Unknown` for bytes outside the named set, so a parsed value is always canonical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireContext {
    None,
    Resource,
    ResourceAdvertisement,
    ResourceRequest,
    ResourceHashUpdate,
    ResourceProof,
    ResourceInitiatorCancel,
    ResourceReceiverCancel,
    CacheRequest,
    Request,
    Response,
    PathResponse,
    Command,
    CommandStatus,
    Channel,
    KeepAlive,
    LinkIdentify,
    LinkClose,
    LinkProof,
    LinkRtt,
    LinkRequestProof,
    Unknown(u8),
}

impl WireContext {
    const fn from_byte(byte: u8) -> Self {
        match byte {
            0x00 => Self::None,
            0x01 => Self::Resource,
            0x02 => Self::ResourceAdvertisement,
            0x03 => Self::ResourceRequest,
            0x04 => Self::ResourceHashUpdate,
            0x05 => Self::ResourceProof,
            0x06 => Self::ResourceInitiatorCancel,
            0x07 => Self::ResourceReceiverCancel,
            0x08 => Self::CacheRequest,
            0x09 => Self::Request,
            0x0A => Self::Response,
            0x0B => Self::PathResponse,
            0x0C => Self::Command,
            0x0D => Self::CommandStatus,
            0x0E => Self::Channel,
            0xFA => Self::KeepAlive,
            0xFB => Self::LinkIdentify,
            0xFC => Self::LinkClose,
            0xFD => Self::LinkProof,
            0xFE => Self::LinkRtt,
            0xFF => Self::LinkRequestProof,
            other => Self::Unknown(other),
        }
    }

    pub const fn to_byte(self) -> u8 {
        match self {
            Self::None => 0x00,
            Self::Resource => 0x01,
            Self::ResourceAdvertisement => 0x02,
            Self::ResourceRequest => 0x03,
            Self::ResourceHashUpdate => 0x04,
            Self::ResourceProof => 0x05,
            Self::ResourceInitiatorCancel => 0x06,
            Self::ResourceReceiverCancel => 0x07,
            Self::CacheRequest => 0x08,
            Self::Request => 0x09,
            Self::Response => 0x0A,
            Self::PathResponse => 0x0B,
            Self::Command => 0x0C,
            Self::CommandStatus => 0x0D,
            Self::Channel => 0x0E,
            Self::KeepAlive => 0xFA,
            Self::LinkIdentify => 0xFB,
            Self::LinkClose => 0xFC,
            Self::LinkProof => 0xFD,
            Self::LinkRtt => 0xFE,
            Self::LinkRequestProof => 0xFF,
            Self::Unknown(byte) => byte,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireAddress([u8; TRUNCATED_HASH_BYTE_LEN]);

impl WireAddress {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationHash([u8; TRUNCATED_HASH_BYTE_LEN]);

impl DestinationHash {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }

    pub const fn from_address(address: WireAddress) -> Self {
        Self(*address.as_bytes())
    }

    pub const fn to_address(&self) -> WireAddress {
        WireAddress(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportId([u8; TRUNCATED_HASH_BYTE_LEN]);

impl TransportId {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

/// Note that `transport_id.is_some()` *is* the Type-1/Type-2 distinction, so the two can never disagree.
///
/// ```text
/// [flags:1][hops:1] ( [transport_id:16] )? [destination:16][context:1] [payload…]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WirePacketHeader {
    pub ifac_flag: IfacFlag,
    pub context_flag: ContextFlag,
    pub propagation: PropagationType,
    pub destination_type: DestinationType,
    pub packet_type: PacketType,
    pub hops: u8,
    pub transport_id: Option<TransportId>,
    pub address: WireAddress,
    pub context: WireContext,
}

impl WirePacketHeader {
    pub fn parse(bytes: &[u8]) -> Result<(WirePacketHeader, &[u8]), WireError> {
        let (&meta, rest) = bytes.split_first().ok_or(WireError::BufferTooShort)?;
        let (&hops, rest) = rest.split_first().ok_or(WireError::BufferTooShort)?;

        let is_type_2 = (meta >> 6) & 0b1 == 0b1;
        let ifac_flag = IfacFlag::from_bits(meta >> 7);
        let context_flag = ContextFlag::from_bits(meta >> 5);
        let propagation = PropagationType::from_bits(meta >> 4);
        let destination_type = DestinationType::from_bits(meta >> 2);
        let packet_type = PacketType::from_bits(meta);

        let (transport_id, rest) = if is_type_2 {
            let (id, rest) = rest.split_first_chunk().ok_or(WireError::BufferTooShort)?;
            (Some(TransportId::new(*id)), rest)
        } else {
            (None, rest)
        };

        let (address, rest) = rest.split_first_chunk().ok_or(WireError::BufferTooShort)?;
        let (&context, rest) = rest.split_first().ok_or(WireError::BufferTooShort)?;

        let header = WirePacketHeader {
            ifac_flag,
            context_flag,
            propagation,
            destination_type,
            packet_type,
            hops,
            transport_id,
            address: WireAddress::new(*address),
            context: WireContext::from_byte(context),
        };
        Ok((header, rest))
    }

    pub fn write(&self, buf: &mut [u8]) -> Result<usize, WireError> {
        let transport_len = if self.transport_id.is_some() {
            TRUNCATED_HASH_BYTE_LEN
        } else {
            0
        };
        let header_len = 2 + transport_len + TRUNCATED_HASH_BYTE_LEN + 1;
        if buf.len() < header_len {
            return Err(WireError::BufferTooShort);
        }

        let header_type_bit = u8::from(self.transport_id.is_some());
        buf[0] = ((self.ifac_flag as u8) << 7)
            | (header_type_bit << 6)
            | ((self.context_flag as u8) << 5)
            | ((self.propagation as u8) << 4)
            | ((self.destination_type as u8) << 2)
            | (self.packet_type as u8);
        buf[1] = self.hops;

        let mut offset = 2;
        if let Some(transport_id) = &self.transport_id {
            buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(transport_id.as_bytes());
            offset += TRUNCATED_HASH_BYTE_LEN;
        }
        buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(self.address.as_bytes());
        offset += TRUNCATED_HASH_BYTE_LEN;
        buf[offset] = self.context.to_byte();
        offset += 1;

        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn bytes_from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn type1_header_round_trips() {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Announce,
            hops: 3,
            transport_id: None,
            address: WireAddress::new([0xAB; TRUNCATED_HASH_BYTE_LEN]),
            context: WireContext::None,
        };

        let mut buf = [0u8; 64];
        let written = header.write(&mut buf).unwrap();
        assert_eq!(written, 2 + TRUNCATED_HASH_BYTE_LEN + 1);

        let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();
        assert_eq!(parsed, header);
        assert!(payload.is_empty());
    }

    #[test]
    fn type2_header_round_trips_with_every_one_bit_set() {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Authenticated,
            context_flag: ContextFlag::Set,
            propagation: PropagationType::Transport,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Proof,
            hops: 7,
            transport_id: Some(TransportId::new([0x11; TRUNCATED_HASH_BYTE_LEN])),
            address: WireAddress::new([0x22; TRUNCATED_HASH_BYTE_LEN]),
            context: WireContext::PathResponse,
        };

        let mut buf = [0u8; 64];
        let written = header.write(&mut buf).unwrap();
        assert_eq!(written, 2 + 2 * TRUNCATED_HASH_BYTE_LEN + 1);

        let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();
        assert_eq!(parsed, header);
        assert!(payload.is_empty());
    }

    #[test]
    fn write_rejects_one_byte_short_header_buffers() {
        let type1 = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Announce,
            hops: 3,
            transport_id: None,
            address: WireAddress::new([0xAB; TRUNCATED_HASH_BYTE_LEN]),
            context: WireContext::None,
        };
        let mut type1_short = [0u8; 2 + TRUNCATED_HASH_BYTE_LEN];
        assert_eq!(
            type1.write(&mut type1_short),
            Err(WireError::BufferTooShort)
        );

        let type2 = WirePacketHeader {
            transport_id: Some(TransportId::new([0x11; TRUNCATED_HASH_BYTE_LEN])),
            address: WireAddress::new([0x22; TRUNCATED_HASH_BYTE_LEN]),
            ..type1
        };
        let mut type2_short = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN];
        assert_eq!(
            type2.write(&mut type2_short),
            Err(WireError::BufferTooShort)
        );
    }

    #[test]
    fn decodes_a_real_rns_announce() {
        let raw = bytes_from_hex(
            "0100e4cd902bf205ffc02a4e1c667afa214e0002cd8c52db77603c33d2c8c11ea852\
             4f2c1caca0f5535b2462045b1b1a683501f8e9bc5442cfbae5e4ca8ec88942e84558\
             f790c0f5f99c78f08d3c0d9e7429f89ab8d12b5e2cafc834dc8d4301deda006a171b\
             768c52c1d010bc5c8c5163940c77c311def1f81e67995ef331edbd848e5cb869badf\
             d4cb7220ee688c3c2817ae0e851909b3afbffcc5a796362a944d1404708f0268656c\
             6c6f2d706572736f6e616c",
        );

        let (header, payload) = WirePacketHeader::parse(&raw).unwrap();

        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Broadcast);
        assert_eq!(header.ifac_flag, IfacFlag::Open);
        assert_eq!(header.context_flag, ContextFlag::Unset);
        assert_eq!(header.context, WireContext::None);
        assert_eq!(header.hops, 0);
        assert_eq!(header.transport_id, None);
        assert_eq!(
            DestinationHash::from_address(header.address),
            DestinationHash::from_slice(&bytes_from_hex("e4cd902bf205ffc02a4e1c667afa214e"))
                .unwrap()
        );
        assert_eq!(payload.len(), 162);

        let mut buf = [0u8; 64];
        let written = header.write(&mut buf).unwrap();
        assert_eq!(written, 19);
        assert_eq!(&buf[..written], &raw[..written]);
    }

    #[test]
    fn every_flags_byte_round_trips_with_unknown_context_and_payload() {
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        for meta in 0u8..=u8::MAX {
            let is_type_2 = meta & 0b0100_0000 != 0;
            let header_len =
                2 + usize::from(is_type_2) * TRUNCATED_HASH_BYTE_LEN + TRUNCATED_HASH_BYTE_LEN + 1;
            let mut raw = vec![0u8; header_len + payload.len()];

            raw[0] = meta;
            raw[1] = 0x7A;
            let mut offset = 2;
            if is_type_2 {
                raw[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(&[0x11; 16]);
                offset += TRUNCATED_HASH_BYTE_LEN;
            }
            raw[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(&[0x22; 16]);
            offset += TRUNCATED_HASH_BYTE_LEN;
            raw[offset] = 0xA5;
            offset += 1;
            raw[offset..].copy_from_slice(&payload);

            let (header, parsed_payload) = WirePacketHeader::parse(&raw).unwrap();
            assert_eq!(header.context, WireContext::Unknown(0xA5));
            assert_eq!(parsed_payload, payload);

            let mut encoded = [0u8; 64];
            let written = header.write(&mut encoded).unwrap();
            assert_eq!(written, header_len);
            assert_eq!(
                &encoded[..written],
                &raw[..header_len],
                "flags {meta:#04x} did not preserve the header bytes",
            );
        }
    }

    #[test]
    fn parse_rejects_truncated_input() {
        assert_eq!(
            WirePacketHeader::parse(&[0x01]),
            Err(WireError::BufferTooShort)
        );
        assert_eq!(
            WirePacketHeader::parse(&[0u8; 18]),
            Err(WireError::BufferTooShort)
        );
        let mut type_2 = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN];
        type_2[0] = 0b0100_0000;
        assert_eq!(
            WirePacketHeader::parse(&type_2),
            Err(WireError::BufferTooShort)
        );
    }

    fn ifac_flags() -> impl Strategy<Value = IfacFlag> {
        prop_oneof![Just(IfacFlag::Open), Just(IfacFlag::Authenticated)]
    }

    fn context_flags() -> impl Strategy<Value = ContextFlag> {
        prop_oneof![Just(ContextFlag::Unset), Just(ContextFlag::Set)]
    }

    fn propagation_types() -> impl Strategy<Value = PropagationType> {
        prop_oneof![
            Just(PropagationType::Broadcast),
            Just(PropagationType::Transport)
        ]
    }

    fn destination_types() -> impl Strategy<Value = DestinationType> {
        prop_oneof![
            Just(DestinationType::Single),
            Just(DestinationType::Group),
            Just(DestinationType::Plain),
            Just(DestinationType::Link)
        ]
    }

    fn packet_types() -> impl Strategy<Value = PacketType> {
        prop_oneof![
            Just(PacketType::Data),
            Just(PacketType::Announce),
            Just(PacketType::LinkRequest),
            Just(PacketType::Proof)
        ]
    }

    fn contexts() -> impl Strategy<Value = WireContext> {
        any::<u8>().prop_map(WireContext::from_byte)
    }

    fn headers() -> impl Strategy<Value = WirePacketHeader> {
        (
            ifac_flags(),
            any::<bool>(),
            context_flags(),
            propagation_types(),
            destination_types(),
            packet_types(),
            any::<u8>(),
            any::<[u8; TRUNCATED_HASH_BYTE_LEN]>(),
            any::<[u8; TRUNCATED_HASH_BYTE_LEN]>(),
            contexts(),
        )
            .prop_map(
                |(
                    ifac_flag,
                    has_transport_id,
                    context_flag,
                    propagation,
                    destination_type,
                    packet_type,
                    hops,
                    transport_id,
                    destination,
                    context,
                )| WirePacketHeader {
                    ifac_flag,
                    context_flag,
                    propagation,
                    destination_type,
                    packet_type,
                    hops,
                    transport_id: has_transport_id.then(|| TransportId::new(transport_id)),
                    address: WireAddress::new(destination),
                    context,
                },
            )
    }

    proptest! {
        #[test]
        fn arbitrary_headers_write_then_parse_back(header in headers()) {
            let mut buf = [0u8; 2 + 2 * TRUNCATED_HASH_BYTE_LEN + 1];
            let written = header.write(&mut buf).unwrap();

            let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();

            prop_assert_eq!(parsed, header);
            prop_assert!(payload.is_empty());
        }
    }
}
