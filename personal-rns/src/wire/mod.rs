//! Wire contract boundary.
//!
//! Packet byte layouts belong here. Internal engine state should use typed
//! values and cross this boundary exactly once when decoding or encoding wire
//! bytes.

pub const TRUNCATED_HASH_BYTE_LEN: usize = 16;

/// RNS's `RNS.Transport.PATHFINDER_M` — packets beyond this hop count are
/// outside reach. A wire-protocol invariant, not a sizing knob.
pub const MAX_HOP_COUNT: u8 = 128;

/// RNS's `RNS.Reticulum.MTU` — the maximum byte size of one Reticulum packet
/// peers must agree on. A wire-protocol invariant; everything that allocates
/// per-packet scratch (announce reassembly, payload buffers) bounds against it.
pub const MTU: usize = 500;

/// 32 B X25519 encryption key + 32 B Ed25519 signing key, concatenated on the
/// wire as one 64-byte field.
pub const ANNOUNCE_PUBLIC_KEY_LEN: usize = 64;

/// Wire width of the dotted app/aspect name hash that prefixes destination
/// binding.
pub const DOTTED_NAME_HASH_LEN: usize = 10;

/// Wire width of the X25519 forward-secrecy ratchet public key carried by
/// announces that opt into ratcheting.
pub const RATCHET_LEN: usize = 32;

/// Wire width of an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// Total bytes of the Reticulum packet header that prefixes every wire
/// packet: 1 byte flags + 1 byte hops + 16-byte destination hash + 1 byte
/// context = 19 bytes. The packet payload starts at this offset.
pub const HEADER_LEN: usize = 2 + TRUNCATED_HASH_BYTE_LEN + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    BufferTooShort,
}

/// Interface access-code flag (flags bit 7). Set by an interface that prepends
/// an IFAC authentication code; an engine sees `Open` once the interface has
/// stripped it. Carried for byte-faithful round-trips, not acted on here.
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

/// Context-flag bit (flags bit 5). Its meaning is packet-type specific — for an
/// announce, `Set` signals that a ratchet public key is present in the payload.
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

/// How a packet propagates (flags bit 4). RNS names this `transport_type`.
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

/// The kind of destination addressed (flags bits 2–3).
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

/// What a packet is for (flags bits 0–1).
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

/// The trailing context byte — a sub-type tag the engine routes on. Exhaustive
/// over the values RNS defines, plus `Unknown(u8)` so an unrecognised byte
/// round-trips unchanged (RNS preserves unknown context bytes). Decoding only
/// ever yields `Unknown` for bytes outside the named set, so a parsed value is
/// always canonical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
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

impl Context {
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

    const fn to_byte(self) -> u8 {
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

/// Where a packet is delivered: a destination's truncated hash. Distinct from
/// [`TransportId`] — same width, different role: a destination names *where to
/// deliver*, never *who relays*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationHash([u8; TRUNCATED_HASH_BYTE_LEN]);

impl DestinationHash {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    /// Returns None if length is incorrect
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

/// The transport node a Type-2 (in-transport) packet is relayed via: its
/// truncated hash. Distinct from [`DestinationHash`] — names *who relays*, and
/// occupies the transport-id slot of a Type-2 header.
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

/// A decoded packet header: the flags byte unpacked into typed fields, the hop
/// count, the destination, and — for Type-2 (in-transport) packets — the
/// transport id. `transport_id.is_some()` *is* the Type-1/Type-2 distinction,
/// so the two can never disagree.
///
/// ```text
/// [flags:1][hops:1] ( [transport_id:16] )? [destination:16][context:1] [payload…]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WirePacketHeader {
    pub ifac_flag: IfacFlag,
    pub context_flag: ContextFlag,
    pub propagation: PropagationType,
    pub destination_type: DestinationType,
    pub packet_type: PacketType,
    pub hops: u8,
    pub transport_id: Option<TransportId>,
    pub destination: DestinationHash,
    pub context: Context,
}

impl WirePacketHeader {
    pub fn parse(bytes: &[u8]) -> Result<(WirePacketHeader, &[u8]), WireError> {
        let meta = *bytes.first().ok_or(WireError::BufferTooShort)?;
        let hops = *bytes.get(1).ok_or(WireError::BufferTooShort)?;

        let is_type_2 = (meta >> 6) & 0b1 == 0b1;
        let ifac_flag = IfacFlag::from_bits(meta >> 7);
        let context_flag = ContextFlag::from_bits(meta >> 5);
        let propagation = PropagationType::from_bits(meta >> 4);
        let destination_type = DestinationType::from_bits(meta >> 2);
        let packet_type = PacketType::from_bits(meta);

        let mut offset = 2;

        let transport_id = if is_type_2 {
            let slot = bytes
                .get(offset..offset + TRUNCATED_HASH_BYTE_LEN)
                .ok_or(WireError::BufferTooShort)?;
            offset += TRUNCATED_HASH_BYTE_LEN;
            Some(TransportId::from_slice(slot).ok_or(WireError::BufferTooShort)?)
        } else {
            None
        };

        let dest_slot = bytes
            .get(offset..offset + TRUNCATED_HASH_BYTE_LEN)
            .ok_or(WireError::BufferTooShort)?;
        offset += TRUNCATED_HASH_BYTE_LEN;
        let destination =
            DestinationHash::from_slice(dest_slot).ok_or(WireError::BufferTooShort)?;

        let context = Context::from_byte(*bytes.get(offset).ok_or(WireError::BufferTooShort)?);
        offset += 1;

        let header = WirePacketHeader {
            ifac_flag,
            context_flag,
            propagation,
            destination_type,
            packet_type,
            hops,
            transport_id,
            destination,
            context,
        };
        Ok((header, &bytes[offset..]))
    }

    /// Encode the header into `buf`, returning the number of bytes written (the
    /// payload, if any, is the caller's to append).
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
        buf[offset..offset + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(self.destination.as_bytes());
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
            destination: DestinationHash::new([0xAB; TRUNCATED_HASH_BYTE_LEN]),
            context: Context::None,
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
            destination: DestinationHash::new([0x22; TRUNCATED_HASH_BYTE_LEN]),
            context: Context::PathResponse,
        };

        let mut buf = [0u8; 64];
        let written = header.write(&mut buf).unwrap();
        assert_eq!(written, 2 + 2 * TRUNCATED_HASH_BYTE_LEN + 1);

        let (parsed, payload) = WirePacketHeader::parse(&buf[..written]).unwrap();
        assert_eq!(parsed, header);
        assert!(payload.is_empty());
    }

    /// Byte-exact decode of a genuine RNS 1.3.1 announce. Generated offline:
    /// `RNS.Destination(Identity(), IN, SINGLE, "personal", "wire_test")
    ///  .announce(app_data=b"hello-personal", send=False)` then `pack()`.
    /// One captured packet (the identity is random per generation).
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
        assert_eq!(header.context, Context::None);
        assert_eq!(header.hops, 0);
        assert_eq!(header.transport_id, None);
        assert_eq!(
            header.destination,
            DestinationHash::from_slice(&bytes_from_hex("e4cd902bf205ffc02a4e1c667afa214e"))
                .unwrap()
        );
        assert_eq!(payload.len(), 162);

        // And the header re-encodes to the exact same 19 leading bytes.
        let mut buf = [0u8; 64];
        let written = header.write(&mut buf).unwrap();
        assert_eq!(written, 19);
        assert_eq!(&buf[..written], &raw[..written]);
    }

    #[test]
    fn every_flags_byte_round_trips_with_unknown_context_and_payload() {
        // The wire boundary should preserve every flag bit and unknown context
        // values exactly; parsed payload bytes stay outside header encoding.
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
            assert_eq!(header.context, Context::Unknown(0xA5));
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
        // A Type-1 header needs 19 bytes; one short must fail.
        assert_eq!(
            WirePacketHeader::parse(&[0u8; 18]),
            Err(WireError::BufferTooShort)
        );
        // A Type-2 header needs the transport-id + destination + context byte;
        // a buffer holding only flags + hops + transport-id + destination is one
        // byte short of the context.
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

    fn contexts() -> impl Strategy<Value = Context> {
        any::<u8>().prop_map(Context::from_byte)
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
                    destination: DestinationHash::new(destination),
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
