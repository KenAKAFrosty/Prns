use crate::interfaces::framing::rns_serial_framing;
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::InterfaceId;
use crate::wire::BROADCAST_MTU;

const PROTOCOL_VERSION_LEN: usize = 1;
const MESSAGE_KIND_LEN: usize = 1;
pub const NODE_TAG_LEN: usize = 8;
pub const MAX_DATA_BYTES: usize = crate::interfaces::IFAC_MAX_SIZE + BROADCAST_MTU;
pub const MAX_MESSAGE_BYTES: usize = MESSAGE_KIND_LEN + MAX_DATA_BYTES;
pub const MAX_FRAMED_BYTES: usize = rns_serial_framing::max_encoded_len(MAX_MESSAGE_BYTES);
pub const READ_CHUNK_BYTES: usize = MAX_FRAMED_BYTES;
pub const MAGIC: [u8; 4] = *b"Prns";
pub const PROTOCOL_VERSION: u8 = 2;
pub const WEBUSB_VENDOR_ID: u16 = 0x1209;
pub const WEBUSB_PRODUCT_ID: u16 = 0x0001;
pub const BOOTLOADER_ENTRY_CONTROL_REQUEST: u8 = 0x50;
pub const BOOTLOADER_ENTRY_CONTROL_VALUE: u16 = 0x5052;
pub const BOOTLOADER_ENTRY_CONTROL_INDEX: u16 = 0x4e53;
pub const ANDROID_ACCESSORY_MANUFACTURER: &str = "Personal";
pub const ANDROID_ACCESSORY_MODEL: &str = "Hopspot";
pub const ANDROID_ACCESSORY_DESCRIPTION: &str = "Prns USB Auto";
pub const ANDROID_ACCESSORY_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ANDROID_ACCESSORY_URI: &str = "https://reticulum.rs";
pub const ANDROID_ACCESSORY_SERIAL: &str = "prns-usb-auto";
const HANDSHAKE_ENVELOPE_LEN: usize = MAGIC.len() + PROTOCOL_VERSION_LEN;
const CAPABILITIES_LEN: usize = 1;
const HELLO_BODY_LEN: usize = HANDSHAKE_ENVELOPE_LEN + CAPABILITIES_LEN;
const HELLO_ACK_BODY_LEN: usize = HANDSHAKE_ENVELOPE_LEN + CAPABILITIES_LEN + NODE_TAG_LEN;

#[repr(u8)]
enum MessageKind {
    Hello = 0x01,
    HelloAck = 0x02,
    Data = 0x03,
}

impl MessageKind {
    fn from_wire(byte: u8) -> Result<Self, MalformedMessage> {
        Ok(match byte {
            b if b == Self::Hello as u8 => Self::Hello,
            b if b == Self::HelloAck as u8 => Self::HelloAck,
            b if b == Self::Data as u8 => Self::Data,
            unknown => return Err(MalformedMessage::UnknownMessageKind { kind_byte: unknown }),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NodeTag(pub [u8; NODE_TAG_LEN]);

pub fn node_tag_for(id: InterfaceId) -> NodeTag {
    let mut tag = [0u8; NODE_TAG_LEN];
    tag.copy_from_slice(&id.as_bytes()[..NODE_TAG_LEN]);
    NodeTag(tag)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capabilities(u8);

impl Capabilities {
    const HOST_LANE: u8 = 0b0000_0001;

    pub const fn none() -> Self {
        Self(0)
    }

    pub const fn host() -> Self {
        Self(Self::HOST_LANE)
    }

    pub const fn supports_host_lane(self) -> bool {
        self.0 & Self::HOST_LANE != 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeerProfile {
    Peripheral,
    Host,
}

impl PeerProfile {
    pub fn negotiate(local: Capabilities, peer: Capabilities) -> Self {
        if local.supports_host_lane() && peer.supports_host_lane() {
            PeerProfile::Host
        } else {
            PeerProfile::Peripheral
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Message<'a> {
    Hello(Capabilities),
    HelloAck {
        tag: NodeTag,
        capabilities: Capabilities,
    },
    Data(&'a [u8]),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteError {
    BufferTooSmall,
    DataTooLarge,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MalformedMessage {
    MissingKindByte,
    UnknownMessageKind { kind_byte: u8 },
    TruncatedHandshake,
    WrongMagic,
    UnsupportedVersion { peer_version: u8 },
    DataTooLarge,
}

impl Message<'_> {
    pub fn write_payload(&self, out: &mut [u8]) -> Result<usize, WriteError> {
        match self {
            Message::Hello(capabilities) => {
                let mut message = [0u8; MESSAGE_KIND_LEN + HELLO_BODY_LEN];
                message[0] = MessageKind::Hello as u8;
                let body = &mut message[MESSAGE_KIND_LEN..];
                write_handshake_envelope(body);
                body[HANDSHAKE_ENVELOPE_LEN] = capabilities.0;
                copy_into(&message, out)
            }
            Message::HelloAck { tag, capabilities } => {
                let mut message = [0u8; MESSAGE_KIND_LEN + HELLO_ACK_BODY_LEN];
                message[0] = MessageKind::HelloAck as u8;
                let body = &mut message[MESSAGE_KIND_LEN..];
                write_handshake_envelope(body);
                body[HANDSHAKE_ENVELOPE_LEN] = capabilities.0;
                body[HANDSHAKE_ENVELOPE_LEN + CAPABILITIES_LEN..].copy_from_slice(&tag.0);
                copy_into(&message, out)
            }
            Message::Data(packet) => {
                if packet.len() > MAX_DATA_BYTES {
                    return Err(WriteError::DataTooLarge);
                }
                let total = MESSAGE_KIND_LEN + packet.len();
                let slot = out.get_mut(..total).ok_or(WriteError::BufferTooSmall)?;
                slot[0] = MessageKind::Data as u8;
                slot[MESSAGE_KIND_LEN..].copy_from_slice(packet);
                Ok(total)
            }
        }
    }

    pub fn write_framed(&self, out: &mut [u8]) -> Result<usize, WriteError> {
        let mut payload = [0u8; MAX_MESSAGE_BYTES];
        let n = self.write_payload(&mut payload)?;
        rns_serial_framing::encode(&payload[..n], out).map_err(|_| WriteError::BufferTooSmall)
    }
}

fn write_handshake_envelope(body: &mut [u8]) {
    body[..MAGIC.len()].copy_from_slice(&MAGIC);
    body[MAGIC.len()] = PROTOCOL_VERSION;
}

fn copy_into(message: &[u8], out: &mut [u8]) -> Result<usize, WriteError> {
    let slot = out
        .get_mut(..message.len())
        .ok_or(WriteError::BufferTooSmall)?;
    slot.copy_from_slice(message);
    Ok(message.len())
}

pub fn decode_message(payload: &[u8]) -> Result<Message<'_>, MalformedMessage> {
    let (&kind_byte, body) = payload
        .split_first()
        .ok_or(MalformedMessage::MissingKindByte)?;
    match MessageKind::from_wire(kind_byte)? {
        MessageKind::Hello => {
            vet_handshake(body, HELLO_BODY_LEN)?;
            Ok(Message::Hello(Capabilities(body[HANDSHAKE_ENVELOPE_LEN])))
        }
        MessageKind::HelloAck => {
            vet_handshake(body, HELLO_ACK_BODY_LEN)?;
            let capabilities = Capabilities(body[HANDSHAKE_ENVELOPE_LEN]);
            let mut node_tag = [0u8; NODE_TAG_LEN];
            node_tag.copy_from_slice(&body[HANDSHAKE_ENVELOPE_LEN + CAPABILITIES_LEN..]);
            Ok(Message::HelloAck {
                tag: NodeTag(node_tag),
                capabilities,
            })
        }
        MessageKind::Data => {
            if body.len() > MAX_DATA_BYTES {
                return Err(MalformedMessage::DataTooLarge);
            }
            Ok(Message::Data(body))
        }
    }
}

fn vet_handshake(body: &[u8], expected_len: usize) -> Result<(), MalformedMessage> {
    if body.len() != expected_len {
        return Err(MalformedMessage::TruncatedHandshake);
    }
    if body[..MAGIC.len()] != MAGIC {
        return Err(MalformedMessage::WrongMagic);
    }
    let peer_version = body[MAGIC.len()];
    if peer_version != PROTOCOL_VERSION {
        return Err(MalformedMessage::UnsupportedVersion { peer_version });
    }
    Ok(())
}

#[cfg(any(test, feature = "embassy-host"))]
pub enum InboundReaction<'a> {
    AnswerHandshake,
    Deliver(&'a [u8]),
    Ignore,
}

#[cfg(any(test, feature = "embassy-host"))]
pub fn react_to(message: Result<Message<'_>, MalformedMessage>) -> InboundReaction<'_> {
    match message {
        Ok(Message::Hello(_)) => InboundReaction::AnswerHandshake,
        Ok(Message::Data(packet)) => InboundReaction::Deliver(packet),
        Ok(Message::HelloAck { .. }) | Err(_) => InboundReaction::Ignore,
    }
}

pub type Decoder = RnsSerialDecoder<MAX_MESSAGE_BYTES>;

pub enum HostInbound<'a> {
    AnswerHandshake,
    Confirmed(NodeTag),
    Data(&'a [u8]),
    Ignore,
}

pub fn host_react(message: Result<Message<'_>, MalformedMessage>) -> HostInbound<'_> {
    match message {
        Ok(Message::Hello(_)) => HostInbound::AnswerHandshake,
        Ok(Message::HelloAck { tag, .. }) => HostInbound::Confirmed(tag),
        Ok(Message::Data(packet)) => HostInbound::Data(packet),
        Err(_) => HostInbound::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
    use proptest::prelude::*;

    #[derive(Clone, PartialEq, Eq, Debug)]
    enum OwnedMessage {
        Hello(Capabilities),
        HelloAck {
            tag: NodeTag,
            capabilities: Capabilities,
        },
        Data(std::vec::Vec<u8>),
    }

    impl OwnedMessage {
        fn as_borrowed(&self) -> Message<'_> {
            match self {
                OwnedMessage::Hello(capabilities) => Message::Hello(*capabilities),
                OwnedMessage::HelloAck { tag, capabilities } => Message::HelloAck {
                    tag: *tag,
                    capabilities: *capabilities,
                },
                OwnedMessage::Data(packet) => Message::Data(packet),
            }
        }

        fn payload_len(&self) -> usize {
            match self {
                OwnedMessage::Hello(_) => MESSAGE_KIND_LEN + HELLO_BODY_LEN,
                OwnedMessage::HelloAck { .. } => MESSAGE_KIND_LEN + HELLO_ACK_BODY_LEN,
                OwnedMessage::Data(packet) => MESSAGE_KIND_LEN + packet.len(),
            }
        }
    }

    fn to_owned(message: Message<'_>) -> OwnedMessage {
        match message {
            Message::Hello(capabilities) => OwnedMessage::Hello(capabilities),
            Message::HelloAck { tag, capabilities } => OwnedMessage::HelloAck { tag, capabilities },
            Message::Data(packet) => OwnedMessage::Data(packet.to_vec()),
        }
    }

    fn capabilities() -> impl Strategy<Value = Capabilities> {
        any::<u8>().prop_map(Capabilities)
    }

    fn node_tags() -> impl Strategy<Value = NodeTag> {
        any::<[u8; NODE_TAG_LEN]>().prop_map(NodeTag)
    }

    fn owned_messages() -> impl Strategy<Value = OwnedMessage> {
        prop_oneof![
            capabilities().prop_map(OwnedMessage::Hello),
            (node_tags(), capabilities())
                .prop_map(|(tag, capabilities)| OwnedMessage::HelloAck { tag, capabilities }),
            prop::collection::vec(any::<u8>(), 0..=MAX_DATA_BYTES).prop_map(OwnedMessage::Data),
        ]
    }

    fn payload_roundtrip(message: Message<'_>) -> Message<'static> {
        let mut buf = [0u8; MAX_MESSAGE_BYTES];
        let n = message.write_payload(&mut buf).expect("write");
        let owned: std::vec::Vec<u8> = buf[..n].to_vec();
        match decode_message(&owned).expect("decode") {
            Message::Hello(capabilities) => Message::Hello(capabilities),
            Message::HelloAck { tag, capabilities } => Message::HelloAck { tag, capabilities },
            Message::Data(_) => unreachable!("test helper not used for data"),
        }
    }

    #[test]
    fn hello_round_trips() {
        assert_eq!(
            payload_roundtrip(Message::Hello(Capabilities::host())),
            Message::Hello(Capabilities::host())
        );
    }

    #[test]
    fn hello_ack_round_trips_with_its_node_tag_and_capabilities() {
        let tag = NodeTag([1, 2, 3, 4, 5, 6, 7, 8]);
        let capabilities = Capabilities::host();
        assert_eq!(
            payload_roundtrip(Message::HelloAck { tag, capabilities }),
            Message::HelloAck { tag, capabilities }
        );
    }

    #[test]
    fn an_unknown_capability_bit_is_preserved_not_rejected() {
        let raw = Capabilities(0b1000_0000 | Capabilities::HOST_LANE);
        let tag = NodeTag([9; NODE_TAG_LEN]);
        assert_eq!(
            payload_roundtrip(Message::HelloAck {
                tag,
                capabilities: raw
            }),
            Message::HelloAck {
                tag,
                capabilities: raw
            }
        );
        assert!(raw.supports_host_lane());
    }

    #[test]
    fn two_capable_hosts_negotiate_the_host_lane() {
        assert_eq!(
            PeerProfile::negotiate(Capabilities::host(), Capabilities::host()),
            PeerProfile::Host
        );
    }

    #[test]
    fn a_peripheral_on_either_side_falls_to_the_peripheral_lane() {
        assert_eq!(
            PeerProfile::negotiate(Capabilities::host(), Capabilities::none()),
            PeerProfile::Peripheral
        );
        assert_eq!(
            PeerProfile::negotiate(Capabilities::none(), Capabilities::host()),
            PeerProfile::Peripheral
        );
    }

    #[test]
    fn data_round_trips() {
        let packet = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut buf = [0u8; MAX_MESSAGE_BYTES];
        let n = Message::Data(&packet).write_payload(&mut buf).unwrap();
        assert_eq!(decode_message(&buf[..n]), Ok(Message::Data(&packet[..])));
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert_eq!(decode_message(&[]), Err(MalformedMessage::MissingKindByte));
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert_eq!(
            decode_message(&[0xFF, 0x00]),
            Err(MalformedMessage::UnknownMessageKind { kind_byte: 0xFF })
        );
    }

    #[test]
    fn handshake_with_wrong_magic_is_rejected() {
        let frame = [
            MessageKind::Hello as u8,
            b'X',
            b'X',
            b'X',
            b'X',
            PROTOCOL_VERSION,
            0,
        ];
        assert_eq!(decode_message(&frame), Err(MalformedMessage::WrongMagic));
    }

    #[test]
    fn handshake_with_unsupported_version_is_rejected() {
        let frame = [
            MessageKind::Hello as u8,
            MAGIC[0],
            MAGIC[1],
            MAGIC[2],
            MAGIC[3],
            PROTOCOL_VERSION + 9,
            0,
        ];
        assert_eq!(
            decode_message(&frame),
            Err(MalformedMessage::UnsupportedVersion {
                peer_version: PROTOCOL_VERSION + 9
            })
        );
    }

    #[test]
    fn truncated_handshake_is_rejected() {
        let frame = [MessageKind::Hello as u8, MAGIC[0], MAGIC[1]];
        assert_eq!(
            decode_message(&frame),
            Err(MalformedMessage::TruncatedHandshake)
        );
    }

    #[test]
    fn oversize_data_is_rejected() {
        let mut frame = std::vec![MessageKind::Data as u8];
        frame.extend(std::iter::repeat_n(0u8, MAX_DATA_BYTES + 1));
        assert_eq!(decode_message(&frame), Err(MalformedMessage::DataTooLarge));
    }

    #[test]
    fn data_at_exactly_the_mtu_is_accepted() {
        let mut frame = std::vec![MessageKind::Data as u8];
        frame.extend(std::iter::repeat_n(0xABu8, MAX_DATA_BYTES));
        match decode_message(&frame).expect("decode") {
            Message::Data(body) => assert_eq!(body.len(), MAX_DATA_BYTES),
            other => panic!("expected data, got {other:?}"),
        }
    }

    #[test]
    fn the_device_answers_a_host_probe() {
        assert!(matches!(
            react_to(Ok(Message::Hello(Capabilities::host()))),
            InboundReaction::AnswerHandshake
        ));
    }

    #[test]
    fn the_device_delivers_a_data_frame_to_the_engine() {
        let packet = [0xDE, 0xAD, 0xBE, 0xEF];
        match react_to(Ok(Message::Data(&packet))) {
            InboundReaction::Deliver(body) => assert_eq!(body, &packet),
            _ => panic!("expected the data frame to be delivered"),
        }
    }

    #[test]
    fn the_device_ignores_a_stray_hello_ack() {
        let tag = NodeTag([9; NODE_TAG_LEN]);
        assert!(matches!(
            react_to(Ok(Message::HelloAck {
                tag,
                capabilities: Capabilities::none()
            })),
            InboundReaction::Ignore
        ));
    }

    #[test]
    fn the_device_ignores_a_malformed_frame() {
        assert!(matches!(
            react_to(Err(MalformedMessage::WrongMagic)),
            InboundReaction::Ignore
        ));
    }

    #[test]
    fn framed_message_survives_the_streaming_decoder() {
        let packet = [0x01, FLAG_LIKE, 0x03, ESC_LIKE, 0x05];
        let mut wire = [0u8; MAX_FRAMED_BYTES];
        let n = Message::Data(&packet).write_framed(&mut wire).unwrap();

        let mut decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES> = RnsSerialDecoder::new();
        let mut decoded = None;
        for &byte in &wire[..n] {
            if let Some(frame) = decoder.feed(byte).unwrap() {
                decoded = Some(decode_message(frame).unwrap().is_data_with(&packet));
            }
        }
        assert_eq!(decoded, Some(true));
    }

    #[test]
    fn a_full_transport_stamped_announce_survives_the_streaming_decoder() {
        let mut packet = [0u8; 238];
        for (i, slot) in packet.iter_mut().enumerate() {
            *slot = (i * 7 + 3) as u8;
        }
        packet[2] = FLAG_LIKE;
        packet[19] = ESC_LIKE;
        packet[200] = FLAG_LIKE;

        let mut wire = [0u8; MAX_FRAMED_BYTES];
        let n = Message::Data(&packet).write_framed(&mut wire).unwrap();

        let mut decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES> = RnsSerialDecoder::new();
        let mut decoded = None;
        for &byte in &wire[..n] {
            if let Some(frame) = decoder.feed(byte).unwrap() {
                decoded = Some(decode_message(frame).unwrap().is_data_with(&packet));
            }
        }
        assert_eq!(decoded, Some(true));
    }

    const FLAG_LIKE: u8 = 0x7E;
    const ESC_LIKE: u8 = 0x7D;

    impl Message<'_> {
        fn is_data_with(&self, expected: &[u8]) -> bool {
            matches!(self, Message::Data(body) if *body == expected)
        }
    }

    proptest! {
        #[test]
        fn arbitrary_messages_round_trip_through_payload_codec(message in owned_messages()) {
            let mut buf = std::vec![0u8; message.payload_len()];
            let written = message.as_borrowed().write_payload(&mut buf).unwrap();

            prop_assert_eq!(written, message.payload_len());
            prop_assert_eq!(to_owned(decode_message(&buf[..written]).unwrap()), message);
        }

        #[test]
        fn arbitrary_messages_fit_exact_payload_buffers_and_reject_one_byte_short(
            message in owned_messages()
        ) {
            let exact_len = message.payload_len();

            let mut exact = std::vec![0u8; exact_len];
            let written = message.as_borrowed().write_payload(&mut exact).unwrap();
            prop_assert_eq!(written, exact_len);

            let mut short = std::vec![0u8; exact_len - 1];
            prop_assert_eq!(
                message.as_borrowed().write_payload(&mut short),
                Err(WriteError::BufferTooSmall)
            );
        }

        #[test]
        fn arbitrary_messages_round_trip_through_framed_byte_stream(
            message in owned_messages(),
            chunk_size in 1usize..32,
        ) {
            let mut wire =
                std::vec![0u8; rns_serial_framing::max_encoded_len(message.payload_len())];
            let n = message.as_borrowed().write_framed(&mut wire).unwrap();

            let mut decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES> = RnsSerialDecoder::new();
            let mut decoded = std::vec::Vec::new();
            for chunk in wire[..n].chunks(chunk_size) {
                decoder.feed_slice(chunk, |frame| {
                    decoded.push(to_owned(decode_message(frame).unwrap()));
                });
            }

            prop_assert_eq!(decoded, std::vec![message]);
        }

        #[test]
        fn arbitrary_capabilities_negotiate_symmetrically(
            local in capabilities(),
            peer in capabilities(),
        ) {
            let expected = if local.supports_host_lane() && peer.supports_host_lane() {
                PeerProfile::Host
            } else {
                PeerProfile::Peripheral
            };

            prop_assert_eq!(PeerProfile::negotiate(local, peer), expected);
            prop_assert_eq!(PeerProfile::negotiate(peer, local), expected);
        }
    }
}
