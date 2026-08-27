use alloc::string::String;
use alloc::vec::Vec;

use rmp::decode::{read_marker, Bytes, RmpRead};
use rmp::Marker;

use crate::identity::IdentityHash;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::BlackholeExpiry;
use crate::units::InstantMillis;
use crate::wire::{DestinationHash, TransportId};

use crate::interfaces::rns_management::{MessagePackEncoder, RnsManagementEncodeError};

use super::wire_names::{argument, data_operation, drop_operation, get, selector};

const REQUEST_MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsInteger(RnsIntegerRepresentation);

#[derive(Debug, Clone, PartialEq, Eq)]
enum RnsIntegerRepresentation {
    Negative(i64),
    Nonnegative(u64),
}

impl RnsInteger {
    pub const fn from_i64(value: i64) -> Self {
        if value < 0 {
            Self(RnsIntegerRepresentation::Negative(value))
        } else {
            Self(RnsIntegerRepresentation::Nonnegative(value as u64))
        }
    }

    pub const fn from_u64(value: u64) -> Self {
        Self(RnsIntegerRepresentation::Nonnegative(value))
    }

    pub const fn nonnegative_value(&self) -> Option<u64> {
        match self.0 {
            RnsIntegerRepresentation::Negative(_) => None,
            RnsIntegerRepresentation::Nonnegative(value) => Some(value),
        }
    }

    pub const fn signed_value(&self) -> Option<i64> {
        match self.0 {
            RnsIntegerRepresentation::Negative(value) => Some(value),
            RnsIntegerRepresentation::Nonnegative(value) => {
                if value <= i64::MAX as u64 {
                    Some(value as i64)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum RnsNumber {
    Integer(RnsInteger),
    Float(f64),
}

impl RnsNumber {
    pub fn blackhole_expiry(&self) -> BlackholeExpiry {
        match self {
            Self::Integer(RnsInteger(RnsIntegerRepresentation::Negative(_))) => {
                BlackholeExpiry::At(InstantMillis(0))
            }
            Self::Integer(RnsInteger(RnsIntegerRepresentation::Nonnegative(0))) => {
                BlackholeExpiry::Indefinite
            }
            Self::Integer(RnsInteger(RnsIntegerRepresentation::Nonnegative(seconds))) => {
                BlackholeExpiry::At(InstantMillis(seconds.saturating_mul(1_000)))
            }
            Self::Float(seconds) if *seconds == 0.0 || seconds.is_nan() => {
                BlackholeExpiry::Indefinite
            }
            Self::Float(seconds) if *seconds < 0.0 => BlackholeExpiry::At(InstantMillis(0)),
            Self::Float(seconds) => {
                let millis = *seconds * 1_000.0;
                let deadline = if !millis.is_finite() || millis >= u64::MAX as f64 {
                    u64::MAX
                } else {
                    millis as u64
                };
                BlackholeExpiry::At(InstantMillis(deadline))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationDataOperation {
    Used,
    Retain,
    Unretain,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PacketHashArgument(Vec<u8>);

impl PacketHashArgument {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn packet_hash(&self) -> Option<PacketHash> {
        let bytes: [u8; PACKET_HASH_LEN] = self.0.as_slice().try_into().ok()?;
        Some(PacketHash::new(bytes))
    }
}

#[derive(Debug, PartialEq)]
pub enum RnsRpcRequest {
    InterfaceStats,
    PathTable {
        max_hops: Option<RnsInteger>,
    },
    RateTable,
    NextHopInterface {
        destination_hash: DestinationHash,
    },
    NextHop {
        destination_hash: DestinationHash,
    },
    FirstHopTimeout {
        destination_hash: DestinationHash,
    },
    LowestInterfaceBitrate,
    MediumPathTimeout,
    LinkCount,
    PacketRssi {
        packet_hash: PacketHashArgument,
    },
    PacketSnr {
        packet_hash: PacketHashArgument,
    },
    PacketQuality {
        packet_hash: PacketHashArgument,
    },
    BlackholedIdentities,
    IsBlackholed {
        identity_hash: IdentityHash,
    },
    DropPath {
        destination_hash: DestinationHash,
    },
    DropAllVia {
        transport_id: TransportId,
    },
    DropAnnounceQueues,
    BlackholeIdentity {
        identity_hash: IdentityHash,
        until: Option<RnsNumber>,
        reason: Option<String>,
    },
    UnblackholeIdentity {
        identity_hash: IdentityHash,
    },
    DestinationData {
        operation: DestinationDataOperation,
        destination_hash: DestinationHash,
    },
    RetainIdentity {
        identity_hash: IdentityHash,
    },
}

impl RnsRpcRequest {
    /// Encode the Python-pickle request payload used by RNS through 1.3.3.
    ///
    /// Shared-instance authentication and framing are common to both dialects;
    /// RNS switched the payload object codec in 1.3.4. Keeping this small
    /// protocol-3 encoder in the wire-contract crate lets host clients fall
    /// back without pulling a Python-specific object model into embedded
    /// builds.
    pub fn encode_pickle(&self) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = PickleRequestEncoder::new();
        match self {
            Self::InterfaceStats => encode_pickle_get(&mut encoder, get::INTERFACE_STATS)?,
            Self::PathTable { max_hops } => {
                encoder.string_field(selector::GET, get::PATH_TABLE)?;
                encoder.field(argument::MAX_HOPS)?;
                encode_pickle_optional_integer(&mut encoder, max_hops);
            }
            Self::RateTable => encode_pickle_get(&mut encoder, get::RATE_TABLE)?,
            Self::NextHopInterface { destination_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::NEXT_HOP_INTERFACE_NAME,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::NextHop { destination_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::NEXT_HOP,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::FirstHopTimeout { destination_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::FIRST_HOP_TIMEOUT,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::LowestInterfaceBitrate => {
                encode_pickle_get(&mut encoder, get::LOWEST_INTERFACE_BITRATE)?
            }
            Self::MediumPathTimeout => encode_pickle_get(&mut encoder, get::MEDIUM_PATH_TIMEOUT)?,
            Self::LinkCount => encode_pickle_get(&mut encoder, get::LINK_COUNT)?,
            Self::PacketRssi { packet_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::PACKET_RSSI,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::PacketSnr { packet_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::PACKET_SNR,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::PacketQuality { packet_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::PACKET_QUALITY,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::BlackholedIdentities => {
                encode_pickle_get(&mut encoder, get::BLACKHOLED_IDENTITIES)?;
            }
            Self::IsBlackholed { identity_hash } => encode_pickle_get_with_binary(
                &mut encoder,
                get::IS_BLACKHOLED,
                argument::IDENTITY_HASH,
                identity_hash.as_bytes(),
            )?,
            Self::DropPath { destination_hash } => encode_pickle_drop_with_binary(
                &mut encoder,
                drop_operation::PATH,
                destination_hash.as_bytes(),
            )?,
            Self::DropAllVia { transport_id } => encode_pickle_drop_with_binary(
                &mut encoder,
                drop_operation::ALL_VIA,
                transport_id.as_bytes(),
            )?,
            Self::DropAnnounceQueues => {
                encoder.string_field(selector::DROP, drop_operation::ANNOUNCE_QUEUES)?;
            }
            Self::BlackholeIdentity {
                identity_hash,
                until,
                reason,
            } => {
                encoder.binary_field(selector::BLACKHOLE_IDENTITY, identity_hash.as_bytes())?;
                encoder.field(argument::UNTIL)?;
                encode_pickle_optional_number(&mut encoder, until);
                encoder.field(argument::REASON)?;
                match reason {
                    Some(reason) => encoder.string(reason)?,
                    None => encoder.nil(),
                }
            }
            Self::UnblackholeIdentity { identity_hash } => {
                encoder.binary_field(selector::UNBLACKHOLE_IDENTITY, identity_hash.as_bytes())?;
            }
            Self::DestinationData {
                operation,
                destination_hash,
            } => {
                encoder.string_field(
                    selector::DESTINATION_DATA,
                    match operation {
                        DestinationDataOperation::Used => data_operation::USED,
                        DestinationDataOperation::Retain => data_operation::RETAIN,
                        DestinationDataOperation::Unretain => data_operation::UNRETAIN,
                    },
                )?;
                encoder.binary_field(argument::DESTINATION_HASH, destination_hash.as_bytes())?;
            }
            Self::RetainIdentity { identity_hash } => {
                encoder.string_field(selector::IDENTITY_DATA, data_operation::RETAIN)?;
                encoder.binary_field(argument::IDENTITY_HASH, identity_hash.as_bytes())?;
            }
        }
        Ok(encoder.finish())
    }

    pub fn encode_message_pack(&self) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        match self {
            Self::InterfaceStats => encode_get(&mut encoder, get::INTERFACE_STATS)?,
            Self::PathTable { max_hops } => {
                encoder.map(2)?;
                encoder.string_field(selector::GET, get::PATH_TABLE)?;
                encoder.field(argument::MAX_HOPS)?;
                encode_optional_integer(&mut encoder, max_hops);
            }
            Self::RateTable => encode_get(&mut encoder, get::RATE_TABLE)?,
            Self::NextHopInterface { destination_hash } => encode_get_with_binary(
                &mut encoder,
                get::NEXT_HOP_INTERFACE_NAME,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::NextHop { destination_hash } => encode_get_with_binary(
                &mut encoder,
                get::NEXT_HOP,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::FirstHopTimeout { destination_hash } => encode_get_with_binary(
                &mut encoder,
                get::FIRST_HOP_TIMEOUT,
                argument::DESTINATION_HASH,
                destination_hash.as_bytes(),
            )?,
            Self::LowestInterfaceBitrate => {
                encode_get(&mut encoder, get::LOWEST_INTERFACE_BITRATE)?
            }
            Self::MediumPathTimeout => encode_get(&mut encoder, get::MEDIUM_PATH_TIMEOUT)?,
            Self::LinkCount => encode_get(&mut encoder, get::LINK_COUNT)?,
            Self::PacketRssi { packet_hash } => encode_get_with_binary(
                &mut encoder,
                get::PACKET_RSSI,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::PacketSnr { packet_hash } => encode_get_with_binary(
                &mut encoder,
                get::PACKET_SNR,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::PacketQuality { packet_hash } => encode_get_with_binary(
                &mut encoder,
                get::PACKET_QUALITY,
                argument::PACKET_HASH,
                packet_hash.as_bytes(),
            )?,
            Self::BlackholedIdentities => encode_get(&mut encoder, get::BLACKHOLED_IDENTITIES)?,
            Self::IsBlackholed { identity_hash } => encode_get_with_binary(
                &mut encoder,
                get::IS_BLACKHOLED,
                argument::IDENTITY_HASH,
                identity_hash.as_bytes(),
            )?,
            Self::DropPath { destination_hash } => encode_drop_with_binary(
                &mut encoder,
                drop_operation::PATH,
                destination_hash.as_bytes(),
            )?,
            Self::DropAllVia { transport_id } => encode_drop_with_binary(
                &mut encoder,
                drop_operation::ALL_VIA,
                transport_id.as_bytes(),
            )?,
            Self::DropAnnounceQueues => {
                encoder.map(1)?;
                encoder.string_field(selector::DROP, drop_operation::ANNOUNCE_QUEUES)?;
            }
            Self::BlackholeIdentity {
                identity_hash,
                until,
                reason,
            } => {
                encoder.map(3)?;
                encoder.field(selector::BLACKHOLE_IDENTITY)?;
                encoder.binary(identity_hash.as_bytes())?;
                encoder.field(argument::UNTIL)?;
                encode_optional_number(&mut encoder, until);
                encoder.field(argument::REASON)?;
                match reason {
                    Some(reason) => encoder.string(reason)?,
                    None => encoder.nil(),
                }
            }
            Self::UnblackholeIdentity { identity_hash } => {
                encoder.map(1)?;
                encoder.field(selector::UNBLACKHOLE_IDENTITY)?;
                encoder.binary(identity_hash.as_bytes())?;
            }
            Self::DestinationData {
                operation,
                destination_hash,
            } => {
                encoder.map(2)?;
                encoder.string_field(
                    selector::DESTINATION_DATA,
                    match operation {
                        DestinationDataOperation::Used => data_operation::USED,
                        DestinationDataOperation::Retain => data_operation::RETAIN,
                        DestinationDataOperation::Unretain => data_operation::UNRETAIN,
                    },
                )?;
                encoder.field(argument::DESTINATION_HASH)?;
                encoder.binary(destination_hash.as_bytes())?;
            }
            Self::RetainIdentity { identity_hash } => {
                encoder.map(2)?;
                encoder.string_field(selector::IDENTITY_DATA, data_operation::RETAIN)?;
                encoder.field(argument::IDENTITY_HASH)?;
                encoder.binary(identity_hash.as_bytes())?;
            }
        }
        Ok(encoder.finish())
    }
}

struct PickleRequestEncoder {
    bytes: Vec<u8>,
}

impl PickleRequestEncoder {
    fn new() -> Self {
        // PROTO 3, EMPTY_DICT, MARK. Protocol 3 is understood by every
        // supported Python 3 RNS release and gives byte strings their native
        // `bytes` representation.
        Self {
            bytes: vec![0x80, 0x03, b'}', b'('],
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.bytes.extend_from_slice(b"u.");
        self.bytes
    }

    fn field(&mut self, name: &str) -> Result<(), RnsManagementEncodeError> {
        self.string(name)
    }

    fn string_field(&mut self, name: &str, value: &str) -> Result<(), RnsManagementEncodeError> {
        self.field(name)?;
        self.string(value)
    }

    fn binary_field(&mut self, name: &str, value: &[u8]) -> Result<(), RnsManagementEncodeError> {
        self.field(name)?;
        self.binary(value)
    }

    fn nil(&mut self) {
        self.bytes.push(b'N');
    }

    fn signed(&mut self, value: i64) {
        self.long(value.to_le_bytes(), value < 0);
    }

    fn unsigned(&mut self, value: u64) {
        self.long(value.to_le_bytes(), false);
    }

    fn long(&mut self, raw: [u8; 8], negative: bool) {
        let extension = if negative { 0xff } else { 0x00 };
        let mut length = raw.len();
        while length > 1
            && raw[length - 1] == extension
            && (raw[length - 2] & 0x80 != 0) == negative
        {
            length -= 1;
        }
        let length_position = self.bytes.len() + 1;
        self.bytes.extend_from_slice(&[0x8a, length as u8]);
        self.bytes.extend_from_slice(&raw[..length]);
        if !negative && raw[length - 1] & 0x80 != 0 {
            // LONG1 uses little-endian two's complement; retain the positive
            // sign when the highest payload bit is set.
            self.bytes[length_position] = (length + 1) as u8;
            self.bytes.push(0);
        }
    }

    fn float(&mut self, value: f64) {
        self.bytes.push(b'G');
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) -> Result<(), RnsManagementEncodeError> {
        let length = u32::try_from(value.len()).map_err(|_| RnsManagementEncodeError)?;
        self.bytes.push(b'X');
        self.bytes.extend_from_slice(&length.to_le_bytes());
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn binary(&mut self, value: &[u8]) -> Result<(), RnsManagementEncodeError> {
        if let Ok(length) = u8::try_from(value.len()) {
            self.bytes.extend_from_slice(&[b'C', length]);
        } else {
            let length = u32::try_from(value.len()).map_err(|_| RnsManagementEncodeError)?;
            self.bytes.push(b'B');
            self.bytes.extend_from_slice(&length.to_le_bytes());
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }
}

fn encode_pickle_get(
    encoder: &mut PickleRequestEncoder,
    operation: &str,
) -> Result<(), RnsManagementEncodeError> {
    encoder.string_field(selector::GET, operation)
}

fn encode_pickle_get_with_binary(
    encoder: &mut PickleRequestEncoder,
    operation: &str,
    argument: &str,
    value: &[u8],
) -> Result<(), RnsManagementEncodeError> {
    encode_pickle_get(encoder, operation)?;
    encoder.binary_field(argument, value)
}

fn encode_pickle_drop_with_binary(
    encoder: &mut PickleRequestEncoder,
    operation: &str,
    value: &[u8],
) -> Result<(), RnsManagementEncodeError> {
    encoder.string_field(selector::DROP, operation)?;
    encoder.binary_field(argument::DESTINATION_HASH, value)
}

fn encode_pickle_optional_integer(encoder: &mut PickleRequestEncoder, value: &Option<RnsInteger>) {
    match value {
        Some(value) => encode_pickle_integer(encoder, value),
        None => encoder.nil(),
    }
}

fn encode_pickle_integer(encoder: &mut PickleRequestEncoder, value: &RnsInteger) {
    match value.0 {
        RnsIntegerRepresentation::Negative(value) => encoder.signed(value),
        RnsIntegerRepresentation::Nonnegative(value) => encoder.unsigned(value),
    }
}

fn encode_pickle_optional_number(encoder: &mut PickleRequestEncoder, value: &Option<RnsNumber>) {
    match value {
        Some(RnsNumber::Integer(value)) => encode_pickle_integer(encoder, value),
        Some(RnsNumber::Float(value)) => encoder.float(*value),
        None => encoder.nil(),
    }
}

fn encode_get(
    encoder: &mut MessagePackEncoder,
    operation: &str,
) -> Result<(), RnsManagementEncodeError> {
    encoder.map(1)?;
    encoder.string_field(selector::GET, operation)?;
    Ok(())
}

fn encode_get_with_binary(
    encoder: &mut MessagePackEncoder,
    operation: &str,
    argument_name: &str,
    value: &[u8],
) -> Result<(), RnsManagementEncodeError> {
    encoder.map(2)?;
    encoder.string_field(selector::GET, operation)?;
    encoder.field(argument_name)?;
    encoder.binary(value)?;
    Ok(())
}

fn encode_drop_with_binary(
    encoder: &mut MessagePackEncoder,
    operation: &str,
    value: &[u8],
) -> Result<(), RnsManagementEncodeError> {
    encoder.map(2)?;
    encoder.string_field(selector::DROP, operation)?;
    encoder.field(argument::DESTINATION_HASH)?;
    encoder.binary(value)?;
    Ok(())
}

fn encode_optional_integer(encoder: &mut MessagePackEncoder, value: &Option<RnsInteger>) {
    match value {
        Some(value) => encode_integer(encoder, value),
        None => encoder.nil(),
    }
}

fn encode_integer(encoder: &mut MessagePackEncoder, value: &RnsInteger) {
    match value.0 {
        RnsIntegerRepresentation::Negative(value) => encoder.signed(value),
        RnsIntegerRepresentation::Nonnegative(value) => encoder.unsigned(value),
    }
}

fn encode_optional_number(encoder: &mut MessagePackEncoder, value: &Option<RnsNumber>) {
    match value {
        Some(RnsNumber::Integer(value)) => encode_integer(encoder, value),
        Some(RnsNumber::Float(value)) => encoder.float(*value),
        None => encoder.nil(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RpcRequestDecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    ExpectedStringKey,
    DuplicateField(String),
    UnknownField(String),
    MissingOperation,
    ContradictoryOperation,
    MissingField(&'static str),
    UnexpectedField(&'static str),
    InvalidFieldType(&'static str),
    InvalidHashLength {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    UnknownOperation {
        selector: &'static str,
        operation: String,
    },
}

type DecodeError = RpcRequestDecodeError;

enum RpcValue {
    Null,
    String(String),
    Binary(Vec<u8>),
    Integer(RnsInteger),
    Float(f64),
    Unsupported,
}

#[derive(Default)]
struct Fields {
    get: Option<RpcValue>,
    drop: Option<RpcValue>,
    blackhole_identity: Option<RpcValue>,
    unblackhole_identity: Option<RpcValue>,
    destination_data: Option<RpcValue>,
    identity_data: Option<RpcValue>,
    max_hops: Option<RpcValue>,
    destination_hash: Option<RpcValue>,
    packet_hash: Option<RpcValue>,
    identity_hash: Option<RpcValue>,
    until: Option<RpcValue>,
    reason: Option<RpcValue>,
}

impl Fields {
    fn operation_count(&self) -> usize {
        [
            self.get.is_some(),
            self.drop.is_some(),
            self.blackhole_identity.is_some(),
            self.unblackhole_identity.is_some(),
            self.destination_data.is_some(),
            self.identity_data.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    fn ensure_only(&self, allowed: &[&str]) -> Result<(), DecodeError> {
        for (name, present) in [
            (selector::GET, self.get.is_some()),
            (selector::DROP, self.drop.is_some()),
            (
                selector::BLACKHOLE_IDENTITY,
                self.blackhole_identity.is_some(),
            ),
            (
                selector::UNBLACKHOLE_IDENTITY,
                self.unblackhole_identity.is_some(),
            ),
            (selector::DESTINATION_DATA, self.destination_data.is_some()),
            (selector::IDENTITY_DATA, self.identity_data.is_some()),
            (argument::MAX_HOPS, self.max_hops.is_some()),
            (argument::DESTINATION_HASH, self.destination_hash.is_some()),
            (argument::PACKET_HASH, self.packet_hash.is_some()),
            (argument::IDENTITY_HASH, self.identity_hash.is_some()),
            (argument::UNTIL, self.until.is_some()),
            (argument::REASON, self.reason.is_some()),
        ] {
            if present && !allowed.contains(&name) {
                return Err(DecodeError::UnexpectedField(name));
            }
        }
        Ok(())
    }
}

pub fn decode(bytes: &[u8]) -> Result<RnsRpcRequest, DecodeError> {
    let mut reader = Bytes::new(bytes);
    let marker = read_marker(&mut reader).map_err(|_| DecodeError::MessagePack)?;
    let Some(field_count) = map_length(marker, &mut reader)? else {
        consume_value_payload(marker, &mut reader, 0)?;
        return if reader.remaining_slice().is_empty() {
            Err(DecodeError::ExpectedMap)
        } else {
            Err(DecodeError::TrailingData)
        };
    };
    let fields = decode_root_fields(&mut reader, field_count)?;
    if !reader.remaining_slice().is_empty() {
        return Err(DecodeError::TrailingData);
    }
    decode_fields(fields)
}

fn decode_root_fields(reader: &mut Bytes<'_>, field_count: usize) -> Result<Fields, DecodeError> {
    let mut fields = Fields::default();
    for _ in 0..field_count {
        let RpcValue::String(key) = decode_value(reader, 1)? else {
            return Err(DecodeError::ExpectedStringKey);
        };
        let value = decode_value(reader, 1)?;
        let slot = match key.as_str() {
            selector::GET => &mut fields.get,
            selector::DROP => &mut fields.drop,
            selector::BLACKHOLE_IDENTITY => &mut fields.blackhole_identity,
            selector::UNBLACKHOLE_IDENTITY => &mut fields.unblackhole_identity,
            selector::DESTINATION_DATA => &mut fields.destination_data,
            selector::IDENTITY_DATA => &mut fields.identity_data,
            argument::MAX_HOPS => &mut fields.max_hops,
            argument::DESTINATION_HASH => &mut fields.destination_hash,
            argument::PACKET_HASH => &mut fields.packet_hash,
            argument::IDENTITY_HASH => &mut fields.identity_hash,
            argument::UNTIL => &mut fields.until,
            argument::REASON => &mut fields.reason,
            _ => return Err(DecodeError::UnknownField(key)),
        };
        if slot.replace(value).is_some() {
            return Err(DecodeError::DuplicateField(key));
        }
    }
    Ok(fields)
}

fn decode_value(reader: &mut Bytes<'_>, depth: usize) -> Result<RpcValue, DecodeError> {
    if depth > REQUEST_MAX_DEPTH {
        return Err(DecodeError::MessagePack);
    }
    let marker = read_marker(reader).map_err(|_| DecodeError::MessagePack)?;
    match marker {
        Marker::FixPos(value) => Ok(RpcValue::Integer(RnsInteger::from_u64(u64::from(value)))),
        Marker::FixNeg(value) => Ok(RpcValue::Integer(RnsInteger::from_i64(i64::from(value)))),
        Marker::Null => Ok(RpcValue::Null),
        Marker::U8 => {
            read_u8(reader).map(|value| RpcValue::Integer(RnsInteger::from_u64(u64::from(value))))
        }
        Marker::U16 => {
            read_u16(reader).map(|value| RpcValue::Integer(RnsInteger::from_u64(u64::from(value))))
        }
        Marker::U32 => {
            read_u32(reader).map(|value| RpcValue::Integer(RnsInteger::from_u64(u64::from(value))))
        }
        Marker::U64 => read_u64(reader).map(|value| RpcValue::Integer(RnsInteger::from_u64(value))),
        Marker::I8 => read_u8(reader)
            .map(|value| RpcValue::Integer(RnsInteger::from_i64(i64::from(value as i8)))),
        Marker::I16 => read_u16(reader)
            .map(|value| RpcValue::Integer(RnsInteger::from_i64(i64::from(value as i16)))),
        Marker::I32 => read_u32(reader)
            .map(|value| RpcValue::Integer(RnsInteger::from_i64(i64::from(value as i32)))),
        Marker::I64 => {
            read_u64(reader).map(|value| RpcValue::Integer(RnsInteger::from_i64(value as i64)))
        }
        Marker::F32 => {
            read_u32(reader).map(|value| RpcValue::Float(f64::from(f32::from_bits(value))))
        }
        Marker::F64 => read_u64(reader).map(|value| RpcValue::Float(f64::from_bits(value))),
        Marker::FixStr(length) => decode_string(reader, usize::from(length)),
        Marker::Str8 => {
            let length = usize::from(read_u8(reader)?);
            decode_string(reader, length)
        }
        Marker::Str16 => {
            let length = usize::from(read_u16(reader)?);
            decode_string(reader, length)
        }
        Marker::Str32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            decode_string(reader, length)
        }
        Marker::Bin8 => {
            let length = usize::from(read_u8(reader)?);
            read_bytes(reader, length).map(RpcValue::Binary)
        }
        Marker::Bin16 => {
            let length = usize::from(read_u16(reader)?);
            read_bytes(reader, length).map(RpcValue::Binary)
        }
        Marker::Bin32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            read_bytes(reader, length).map(RpcValue::Binary)
        }
        Marker::Reserved => Err(DecodeError::MessagePack),
        marker => {
            consume_value_payload(marker, reader, depth)?;
            Ok(RpcValue::Unsupported)
        }
    }
}

fn consume_value_payload(
    marker: Marker,
    reader: &mut Bytes<'_>,
    depth: usize,
) -> Result<(), DecodeError> {
    if depth > REQUEST_MAX_DEPTH {
        return Err(DecodeError::MessagePack);
    }
    match marker {
        Marker::False | Marker::True | Marker::Null | Marker::FixPos(_) | Marker::FixNeg(_) => {}
        Marker::U8 | Marker::I8 => skip_bytes(reader, 1)?,
        Marker::U16 | Marker::I16 => skip_bytes(reader, 2)?,
        Marker::U32 | Marker::I32 | Marker::F32 => skip_bytes(reader, 4)?,
        Marker::U64 | Marker::I64 | Marker::F64 => skip_bytes(reader, 8)?,
        Marker::FixStr(length) => skip_bytes(reader, usize::from(length))?,
        Marker::Str8 | Marker::Bin8 => {
            let length = usize::from(read_u8(reader)?);
            skip_bytes(reader, length)?;
        }
        Marker::Str16 | Marker::Bin16 => {
            let length = usize::from(read_u16(reader)?);
            skip_bytes(reader, length)?;
        }
        Marker::Str32 | Marker::Bin32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            skip_bytes(reader, length)?;
        }
        Marker::FixArray(length) => consume_sequence(reader, usize::from(length), depth)?,
        Marker::Array16 => {
            let length = usize::from(read_u16(reader)?);
            consume_sequence(reader, length, depth)?;
        }
        Marker::Array32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            consume_sequence(reader, length, depth)?;
        }
        Marker::FixMap(length) => consume_map(reader, usize::from(length), depth)?,
        Marker::Map16 => {
            let length = usize::from(read_u16(reader)?);
            consume_map(reader, length, depth)?;
        }
        Marker::Map32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            consume_map(reader, length, depth)?;
        }
        Marker::FixExt1 => skip_bytes(reader, 2)?,
        Marker::FixExt2 => skip_bytes(reader, 3)?,
        Marker::FixExt4 => skip_bytes(reader, 5)?,
        Marker::FixExt8 => skip_bytes(reader, 9)?,
        Marker::FixExt16 => skip_bytes(reader, 17)?,
        Marker::Ext8 => {
            let length = usize::from(read_u8(reader)?);
            skip_bytes(
                reader,
                length.checked_add(1).ok_or(DecodeError::MessagePack)?,
            )?;
        }
        Marker::Ext16 => {
            let length = usize::from(read_u16(reader)?);
            skip_bytes(
                reader,
                length.checked_add(1).ok_or(DecodeError::MessagePack)?,
            )?;
        }
        Marker::Ext32 => {
            let length =
                usize::try_from(read_u32(reader)?).map_err(|_| DecodeError::MessagePack)?;
            skip_bytes(
                reader,
                length.checked_add(1).ok_or(DecodeError::MessagePack)?,
            )?;
        }
        Marker::Reserved => return Err(DecodeError::MessagePack),
    }
    Ok(())
}

fn consume_sequence(
    reader: &mut Bytes<'_>,
    length: usize,
    depth: usize,
) -> Result<(), DecodeError> {
    for _ in 0..length {
        decode_value(reader, depth + 1)?;
    }
    Ok(())
}

fn consume_map(reader: &mut Bytes<'_>, length: usize, depth: usize) -> Result<(), DecodeError> {
    for _ in 0..length {
        decode_value(reader, depth + 1)?;
        decode_value(reader, depth + 1)?;
    }
    Ok(())
}

fn map_length(marker: Marker, reader: &mut Bytes<'_>) -> Result<Option<usize>, DecodeError> {
    match marker {
        Marker::FixMap(length) => Ok(Some(usize::from(length))),
        Marker::Map16 => Ok(Some(usize::from(read_u16(reader)?))),
        Marker::Map32 => usize::try_from(read_u32(reader)?)
            .map(Some)
            .map_err(|_| DecodeError::MessagePack),
        _ => Ok(None),
    }
}

fn decode_string(reader: &mut Bytes<'_>, length: usize) -> Result<RpcValue, DecodeError> {
    let bytes = read_bytes(reader, length)?;
    Ok(String::from_utf8(bytes).map_or(RpcValue::Unsupported, RpcValue::String))
}

fn read_bytes(reader: &mut Bytes<'_>, length: usize) -> Result<Vec<u8>, DecodeError> {
    if reader.remaining_slice().len() < length {
        return Err(DecodeError::MessagePack);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| DecodeError::MessagePack)?;
    bytes.resize(length, 0);
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| DecodeError::MessagePack)?;
    Ok(bytes)
}

fn skip_bytes<'a>(reader: &mut Bytes<'a>, length: usize) -> Result<(), DecodeError> {
    let remaining = reader.remaining_slice();
    let after = remaining.get(length..).ok_or(DecodeError::MessagePack)?;
    *reader = Bytes::new(after);
    Ok(())
}

fn read_u8(reader: &mut Bytes<'_>) -> Result<u8, DecodeError> {
    reader.read_u8().map_err(|_| DecodeError::MessagePack)
}

fn read_u16(reader: &mut Bytes<'_>) -> Result<u16, DecodeError> {
    let mut bytes = [0u8; 2];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| DecodeError::MessagePack)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(reader: &mut Bytes<'_>) -> Result<u32, DecodeError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| DecodeError::MessagePack)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut Bytes<'_>) -> Result<u64, DecodeError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact_buf(&mut bytes)
        .map_err(|_| DecodeError::MessagePack)?;
    Ok(u64::from_be_bytes(bytes))
}

fn decode_fields(fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    match fields.operation_count() {
        0 => Err(DecodeError::MissingOperation),
        2.. => Err(DecodeError::ContradictoryOperation),
        _ if fields.get.is_some() => decode_get(fields),
        _ if fields.drop.is_some() => decode_drop(fields),
        _ if fields.blackhole_identity.is_some() => decode_blackhole(fields),
        _ if fields.unblackhole_identity.is_some() => decode_unblackhole(fields),
        _ if fields.destination_data.is_some() => decode_destination_data(fields),
        _ => decode_identity_data(fields),
    }
}

fn decode_get(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    let operation = take_string(&mut fields.get, selector::GET)?;
    match operation.as_str() {
        get::INTERFACE_STATS => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::InterfaceStats)
        }
        get::PATH_TABLE => {
            fields.ensure_only(&[selector::GET, argument::MAX_HOPS])?;
            let max_hops = take_optional_integer(&mut fields.max_hops, argument::MAX_HOPS)?;
            Ok(RnsRpcRequest::PathTable { max_hops })
        }
        get::RATE_TABLE => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::RateTable)
        }
        get::NEXT_HOP_INTERFACE_NAME => {
            fields.ensure_only(&[selector::GET, argument::DESTINATION_HASH])?;
            Ok(RnsRpcRequest::NextHopInterface {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        get::NEXT_HOP => {
            fields.ensure_only(&[selector::GET, argument::DESTINATION_HASH])?;
            Ok(RnsRpcRequest::NextHop {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        get::FIRST_HOP_TIMEOUT => {
            fields.ensure_only(&[selector::GET, argument::DESTINATION_HASH])?;
            Ok(RnsRpcRequest::FirstHopTimeout {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        get::LOWEST_INTERFACE_BITRATE => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::LowestInterfaceBitrate)
        }
        get::MEDIUM_PATH_TIMEOUT => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::MediumPathTimeout)
        }
        get::LINK_COUNT => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::LinkCount)
        }
        get::PACKET_RSSI => {
            fields.ensure_only(&[selector::GET, argument::PACKET_HASH])?;
            Ok(RnsRpcRequest::PacketRssi {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        get::PACKET_SNR => {
            fields.ensure_only(&[selector::GET, argument::PACKET_HASH])?;
            Ok(RnsRpcRequest::PacketSnr {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        get::PACKET_QUALITY => {
            fields.ensure_only(&[selector::GET, argument::PACKET_HASH])?;
            Ok(RnsRpcRequest::PacketQuality {
                packet_hash: take_packet_hash(&mut fields.packet_hash)?,
            })
        }
        get::BLACKHOLED_IDENTITIES => {
            fields.ensure_only(&[selector::GET])?;
            Ok(RnsRpcRequest::BlackholedIdentities)
        }
        get::IS_BLACKHOLED => {
            fields.ensure_only(&[selector::GET, argument::IDENTITY_HASH])?;
            Ok(RnsRpcRequest::IsBlackholed {
                identity_hash: take_identity_hash(&mut fields.identity_hash)?,
            })
        }
        _ => Err(DecodeError::UnknownOperation {
            selector: selector::GET,
            operation,
        }),
    }
}

fn decode_drop(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    let operation = take_string(&mut fields.drop, selector::DROP)?;
    match operation.as_str() {
        drop_operation::PATH => {
            fields.ensure_only(&[selector::DROP, argument::DESTINATION_HASH])?;
            Ok(RnsRpcRequest::DropPath {
                destination_hash: take_destination_hash(&mut fields.destination_hash)?,
            })
        }
        drop_operation::ALL_VIA => {
            fields.ensure_only(&[selector::DROP, argument::DESTINATION_HASH])?;
            Ok(RnsRpcRequest::DropAllVia {
                transport_id: take_transport_id(&mut fields.destination_hash)?,
            })
        }
        drop_operation::ANNOUNCE_QUEUES => {
            fields.ensure_only(&[selector::DROP])?;
            Ok(RnsRpcRequest::DropAnnounceQueues)
        }
        _ => Err(DecodeError::UnknownOperation {
            selector: selector::DROP,
            operation,
        }),
    }
}

fn decode_blackhole(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&[
        selector::BLACKHOLE_IDENTITY,
        argument::UNTIL,
        argument::REASON,
    ])?;
    Ok(RnsRpcRequest::BlackholeIdentity {
        identity_hash: take_identity_hash(&mut fields.blackhole_identity)?,
        until: take_optional_number(&mut fields.until, argument::UNTIL)?,
        reason: take_optional_string(&mut fields.reason, argument::REASON)?,
    })
}

fn decode_unblackhole(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&[selector::UNBLACKHOLE_IDENTITY])?;
    Ok(RnsRpcRequest::UnblackholeIdentity {
        identity_hash: take_identity_hash(&mut fields.unblackhole_identity)?,
    })
}

fn decode_destination_data(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&[selector::DESTINATION_DATA, argument::DESTINATION_HASH])?;
    let operation =
        match take_string(&mut fields.destination_data, selector::DESTINATION_DATA)?.as_str() {
            data_operation::USED => DestinationDataOperation::Used,
            data_operation::RETAIN => DestinationDataOperation::Retain,
            data_operation::UNRETAIN => DestinationDataOperation::Unretain,
            operation => {
                return Err(DecodeError::UnknownOperation {
                    selector: selector::DESTINATION_DATA,
                    operation: operation.into(),
                });
            }
        };
    Ok(RnsRpcRequest::DestinationData {
        operation,
        destination_hash: take_destination_hash(&mut fields.destination_hash)?,
    })
}

fn decode_identity_data(mut fields: Fields) -> Result<RnsRpcRequest, DecodeError> {
    fields.ensure_only(&[selector::IDENTITY_DATA, argument::IDENTITY_HASH])?;
    let operation = take_string(&mut fields.identity_data, selector::IDENTITY_DATA)?;
    if operation != data_operation::RETAIN {
        return Err(DecodeError::UnknownOperation {
            selector: selector::IDENTITY_DATA,
            operation,
        });
    }
    Ok(RnsRpcRequest::RetainIdentity {
        identity_hash: take_identity_hash(&mut fields.identity_hash)?,
    })
}

fn take_required(
    slot: &mut Option<RpcValue>,
    field: &'static str,
) -> Result<RpcValue, DecodeError> {
    slot.take().ok_or(DecodeError::MissingField(field))
}

fn take_string(slot: &mut Option<RpcValue>, field: &'static str) -> Result<String, DecodeError> {
    match take_required(slot, field)? {
        RpcValue::String(value) => Ok(value),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_string(
    slot: &mut Option<RpcValue>,
    field: &'static str,
) -> Result<Option<String>, DecodeError> {
    match take_required(slot, field)? {
        RpcValue::Null => Ok(None),
        RpcValue::String(value) => Ok(Some(value)),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_integer(
    slot: &mut Option<RpcValue>,
    field: &'static str,
) -> Result<Option<RnsInteger>, DecodeError> {
    match take_required(slot, field)? {
        RpcValue::Null => Ok(None),
        RpcValue::Integer(value) => Ok(Some(value)),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_optional_number(
    slot: &mut Option<RpcValue>,
    field: &'static str,
) -> Result<Option<RnsNumber>, DecodeError> {
    match take_required(slot, field)? {
        RpcValue::Null => Ok(None),
        RpcValue::Integer(value) => Ok(Some(RnsNumber::Integer(value))),
        RpcValue::Float(value) => Ok(Some(RnsNumber::Float(value))),
        _ => Err(DecodeError::InvalidFieldType(field)),
    }
}

fn take_destination_hash(slot: &mut Option<RpcValue>) -> Result<DestinationHash, DecodeError> {
    take_binary::<16>(slot, argument::DESTINATION_HASH).map(DestinationHash::new)
}

fn take_transport_id(slot: &mut Option<RpcValue>) -> Result<TransportId, DecodeError> {
    take_binary::<16>(slot, argument::DESTINATION_HASH).map(TransportId::new)
}

fn take_identity_hash(slot: &mut Option<RpcValue>) -> Result<IdentityHash, DecodeError> {
    take_binary::<16>(slot, argument::IDENTITY_HASH).map(IdentityHash::new)
}

fn take_packet_hash(slot: &mut Option<RpcValue>) -> Result<PacketHashArgument, DecodeError> {
    let RpcValue::Binary(bytes) = take_required(slot, argument::PACKET_HASH)? else {
        return Err(DecodeError::InvalidFieldType(argument::PACKET_HASH));
    };
    Ok(PacketHashArgument(bytes))
}

fn take_binary<const N: usize>(
    slot: &mut Option<RpcValue>,
    field: &'static str,
) -> Result<[u8; N], DecodeError> {
    let RpcValue::Binary(bytes) = take_required(slot, field)? else {
        return Err(DecodeError::InvalidFieldType(field));
    };
    let actual = bytes.len();
    bytes
        .try_into()
        .map_err(|_| DecodeError::InvalidHashLength {
            field,
            expected: N,
            actual,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::shared_instance::rns_rpc::EncodedRpcFrameHeader;
    use rmpv::Value;
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::Command;

    fn request(entries: Vec<(&str, Value)>) -> Vec<u8> {
        let value = Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (Value::from(key), value))
                .collect(),
        );
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &value).unwrap();
        bytes
    }

    #[test]
    fn pickle_encoder_preserves_its_container_and_binary_field_bytes() {
        let mut encoder = PickleRequestEncoder::new();
        encoder
            .binary_field(argument::DESTINATION_HASH, &[0x42; 16])
            .unwrap();
        let encoded = encoder.finish();

        let mut expected = vec![0x80, 0x03, b'}', b'(', b'X'];
        expected.extend_from_slice(&(argument::DESTINATION_HASH.len() as u32).to_le_bytes());
        expected.extend_from_slice(argument::DESTINATION_HASH.as_bytes());
        expected.extend_from_slice(&[b'C', 16]);
        expected.extend_from_slice(&[0x42; 16]);
        expected.extend_from_slice(b"u.");
        assert_eq!(encoded, expected);
    }

    #[test]
    fn pickle_encoder_preserves_signed_and_unsigned_long_boundaries() {
        let mut encoder = PickleRequestEncoder::new();
        encoder.signed(-129);
        encoder.signed(-128);
        encoder.signed(-1);
        encoder.unsigned(0);
        encoder.unsigned(127);
        encoder.unsigned(128);
        encoder.unsigned(u64::MAX);
        encoder.nil();
        encoder.float(123.5);
        let float = 123.5f64.to_be_bytes();
        let mut expected = vec![
            0x80, 0x03, b'}', b'(', 0x8A, 2, 0x7F, 0xFF, 0x8A, 1, 0x80, 0x8A, 1, 0xFF, 0x8A, 1,
            0x00, 0x8A, 1, 0x7F, 0x8A, 2, 0x80, 0x00, 0x8A, 9, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0x00, b'N', b'G',
        ];
        expected.extend_from_slice(&float);
        expected.extend_from_slice(b"u.");
        assert_eq!(encoder.finish(), expected,);
    }

    #[test]
    fn pickle_encoder_switches_binary_width_at_256_bytes() {
        let mut encoder = PickleRequestEncoder::new();
        encoder.binary(&vec![0xAA; 255]).unwrap();
        encoder.binary(&vec![0xBB; 256]).unwrap();
        let encoded = encoder.finish();
        assert_eq!(&encoded[..6], &[0x80, 0x03, b'}', b'(', b'C', 0xFF]);
        assert_eq!(&encoded[6..261], &[0xAA; 255]);
        assert_eq!(&encoded[261..266], &[b'B', 0x00, 0x01, 0x00, 0x00]);
        assert_eq!(&encoded[266..522], &[0xBB; 256]);
        assert_eq!(&encoded[522..], b"u.");
    }

    #[test]
    fn pickle_request_encoding_preserves_verb_and_destination() {
        let destination_hash = DestinationHash::new([0x42; 16]);
        for (request, verb) in [
            (
                RnsRpcRequest::NextHop { destination_hash },
                super::super::RpcVerb::GetNextHop,
            ),
            (
                RnsRpcRequest::DropPath { destination_hash },
                super::super::RpcVerb::DropPath,
            ),
        ] {
            let encoded = request.encode_pickle().unwrap();
            let decoded = super::super::RpcRequest::decode(&encoded).unwrap();
            assert_eq!(decoded.dialect(), super::super::RpcDialect::Pickle);
            assert_eq!(decoded.verb(), verb);
            assert_eq!(decoded.legacy_destination_hash(), Some(destination_hash));
        }

        let encoded = RnsRpcRequest::PathTable { max_hops: None }
            .encode_pickle()
            .unwrap();
        assert!(encoded.contains(&b'N'));

        let encoded = RnsRpcRequest::PathTable {
            max_hops: Some(RnsInteger::from_i64(-1)),
        }
        .encode_pickle()
        .unwrap();
        assert!(encoded.windows(3).any(|window| window == [0x8A, 1, 0xFF]));

        let encoded = RnsRpcRequest::BlackholeIdentity {
            identity_hash: IdentityHash::new([0x55; 16]),
            until: Some(RnsNumber::Float(123.5)),
            reason: None,
        }
        .encode_pickle()
        .unwrap();
        let mut packed_float = vec![b'G'];
        packed_float.extend_from_slice(&123.5f64.to_be_bytes());
        assert!(encoded
            .windows(packed_float.len())
            .any(|window| window == packed_float));
    }

    #[test]
    fn bitrate_timing_operations_round_trip_in_both_rpc_dialects() {
        for (request, verb) in [
            (
                RnsRpcRequest::LowestInterfaceBitrate,
                super::super::RpcVerb::GetLowestInterfaceBitrate,
            ),
            (
                RnsRpcRequest::MediumPathTimeout,
                super::super::RpcVerb::GetMediumPathTimeout,
            ),
        ] {
            let message_pack = request.encode_message_pack().unwrap();
            let pickle = request.encode_pickle().unwrap();
            let decoded = super::super::RpcRequest::decode(&pickle).unwrap();
            assert_eq!(decoded.dialect(), super::super::RpcDialect::Pickle);
            assert_eq!(decoded.verb(), verb);
            assert_eq!(decode(&message_pack), Ok(request));
        }
    }

    fn binary<const N: usize>(byte: u8) -> Value {
        Value::Binary(vec![byte; N])
    }

    fn decode_hex(hex: &str) -> Vec<u8> {
        assert!(hex.len().is_multiple_of(2));
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("valid oracle hex"))
            .collect()
    }

    fn reference_python() -> Option<OsString> {
        if std::env::var("PRNS_VALIDATION_SUITE").as_deref() != Ok("oracle-rpc-codec") {
            return None;
        }
        let interpreter = std::env::var_os("RPC_SMOKE_PYTHON")
            .expect("the oracle-rpc-codec suite must provide RPC_SMOKE_PYTHON");
        assert!(
            !interpreter.is_empty(),
            "RPC_SMOKE_PYTHON must name a Python interpreter"
        );
        Some(interpreter)
    }

    fn rpc_oracle() -> Option<serde_json::Value> {
        let python = reference_python()?;
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../validation/oracles/python/rpc_oracle.py");
        let output = Command::new(python)
            .arg(script)
            .output()
            .expect("spawn Python RPC oracle");
        assert!(
            output.status.success(),
            "Python RPC oracle failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(serde_json::from_slice(&output.stdout).expect("Python RPC oracle emits JSON"))
    }

    #[test]
    fn stock_public_methods_emit_the_complete_canonical_rpc_surface() {
        let Some(oracle) = rpc_oracle() else {
            return;
        };
        let destination_hash = DestinationHash::new([0x11; 16]);
        let packet_hash = PacketHashArgument::new(vec![0x22; 32]);
        let identity_hash = IdentityHash::new([0x33; 16]);
        let expected = vec![
            ("interface_stats", RnsRpcRequest::InterfaceStats),
            (
                "path_table",
                RnsRpcRequest::PathTable {
                    max_hops: Some(RnsInteger::from_u64(8)),
                },
            ),
            ("rate_table", RnsRpcRequest::RateTable),
            (
                "next_hop_if_name",
                RnsRpcRequest::NextHopInterface { destination_hash },
            ),
            ("next_hop", RnsRpcRequest::NextHop { destination_hash }),
            (
                "first_hop_timeout",
                RnsRpcRequest::FirstHopTimeout { destination_hash },
            ),
            ("link_count", RnsRpcRequest::LinkCount),
            (
                "packet_rssi",
                RnsRpcRequest::PacketRssi {
                    packet_hash: PacketHashArgument::new(packet_hash.as_bytes().to_vec()),
                },
            ),
            (
                "packet_snr",
                RnsRpcRequest::PacketSnr {
                    packet_hash: PacketHashArgument::new(packet_hash.as_bytes().to_vec()),
                },
            ),
            ("packet_q", RnsRpcRequest::PacketQuality { packet_hash }),
            ("blackholed_identities", RnsRpcRequest::BlackholedIdentities),
            (
                "is_blackholed",
                RnsRpcRequest::IsBlackholed { identity_hash },
            ),
            ("drop_path", RnsRpcRequest::DropPath { destination_hash }),
            (
                "drop_all_via",
                RnsRpcRequest::DropAllVia {
                    transport_id: TransportId::new([0x11; 16]),
                },
            ),
            ("drop_announce_queues", RnsRpcRequest::DropAnnounceQueues),
            (
                "blackhole_identity",
                RnsRpcRequest::BlackholeIdentity {
                    identity_hash,
                    until: Some(RnsNumber::Integer(RnsInteger::from_u64(2_147_483_648))),
                    reason: Some("oracle".into()),
                },
            ),
            (
                "unblackhole_identity",
                RnsRpcRequest::UnblackholeIdentity { identity_hash },
            ),
            (
                "destination_data_used",
                RnsRpcRequest::DestinationData {
                    operation: DestinationDataOperation::Used,
                    destination_hash,
                },
            ),
            (
                "destination_data_retain",
                RnsRpcRequest::DestinationData {
                    operation: DestinationDataOperation::Retain,
                    destination_hash,
                },
            ),
            (
                "destination_data_unretain",
                RnsRpcRequest::DestinationData {
                    operation: DestinationDataOperation::Unretain,
                    destination_hash,
                },
            ),
            (
                "identity_data_retain",
                RnsRpcRequest::RetainIdentity { identity_hash },
            ),
        ];
        let canonical = oracle["canonical"].as_array().expect("canonical array");
        assert_eq!(canonical.len(), 21);
        assert_eq!(expected.len(), canonical.len());
        for ((name, expected), captured) in expected.into_iter().zip(canonical) {
            assert_eq!(captured["name"].as_str(), Some(name));
            let bytes = decode_hex(captured["hex"].as_str().expect("canonical hex"));
            let decoded = decode(&bytes)
                .unwrap_or_else(|error| panic!("stock request {name} did not decode: {error:?}"));
            assert_eq!(decoded, expected, "stock request {name}");
            assert_eq!(
                decoded.encode_message_pack().unwrap(),
                bytes,
                "stock request {name} did not re-encode byte-identically"
            );
        }
    }

    #[test]
    fn stock_generated_rpc_mutations_are_rejected_without_panics() {
        let Some(oracle) = rpc_oracle() else {
            return;
        };
        for mutation in oracle["mutations"].as_array().expect("mutation array") {
            let name = mutation["name"].as_str().expect("mutation name");
            let bytes = decode_hex(mutation["hex"].as_str().expect("mutation hex"));
            assert!(
                decode(&bytes).is_err(),
                "Python mutation {name} unexpectedly decoded: {}",
                mutation["hex"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn stock_and_prns_preserve_all_msgpack_integer_boundaries() {
        let Some(oracle) = rpc_oracle() else {
            return;
        };
        for boundary in oracle["integer_boundaries"]
            .as_array()
            .expect("integer boundary array")
        {
            let value = boundary["value"].as_str().expect("integer value");
            let bytes = decode_hex(boundary["hex"].as_str().expect("integer hex"));
            let expected = if value.starts_with('-') {
                RnsInteger::from_i64(value.parse().expect("signed boundary"))
            } else {
                RnsInteger::from_u64(value.parse().expect("unsigned boundary"))
            };
            let decoded = decode(&bytes).expect("stock integer boundary decodes");
            assert_eq!(
                decoded,
                RnsRpcRequest::PathTable {
                    max_hops: Some(expected)
                },
                "stock integer boundary {value}"
            );
            assert_eq!(decoded.encode_message_pack().unwrap(), bytes);
        }
    }

    #[test]
    fn cpython_and_prns_agree_on_frame_header_boundaries() {
        let Some(oracle) = rpc_oracle() else {
            return;
        };
        assert_eq!(
            oracle["request_frame_max_length"].as_u64(),
            Some(crate::interfaces::shared_instance::rns_rpc::RPC_FRAME_MAX_LENGTH as u64)
        );
        for header in oracle["headers"].as_array().expect("header array") {
            let length = header["length"]
                .as_str()
                .expect("header length")
                .parse::<usize>()
                .expect("platform header length");
            let expected = decode_hex(header["hex"].as_str().expect("header hex"));
            let actual = EncodedRpcFrameHeader::new(length).expect("length fits the RPC wire");
            assert_eq!(actual.as_bytes(), expected, "frame length {length}");
        }
    }

    #[test]
    fn map_lengths_decode_fixmap_map16_and_map32_headers() {
        assert_eq!(
            map_length(Marker::FixMap(15), &mut Bytes::new(&[])).unwrap(),
            Some(15),
        );
        assert_eq!(
            map_length(Marker::Map16, &mut Bytes::new(&[0x01, 0x00])).unwrap(),
            Some(256),
        );
        assert_eq!(
            map_length(Marker::Map32, &mut Bytes::new(&[0x00, 0x01, 0x00, 0x00]),).unwrap(),
            Some(65_536),
        );
        assert_eq!(
            map_length(Marker::Null, &mut Bytes::new(&[])).unwrap(),
            None,
        );
    }

    #[test]
    fn unsupported_payload_consumption_advances_and_refuses_truncation() {
        let mut reader = Bytes::new(&[0xCA, 0x01, 0x02, 0x03, 0x04, 0xC0]);
        assert!(decode_value(&mut reader, 0).is_ok());
        assert_eq!(reader.remaining_slice(), &[0xC0]);

        let mut reader = Bytes::new(&[0xCA, 0x01, 0x02, 0x03]);
        assert!(matches!(
            decode_value(&mut reader, 0),
            Err(DecodeError::MessagePack),
        ));

        let mut reader = Bytes::new(&[0x01, 0x02, 0xC0]);
        assert!(consume_value_payload(Marker::FixExt1, &mut reader, 0).is_ok());
        assert_eq!(reader.remaining_slice(), &[0xC0]);

        let mut reader = Bytes::new(&[0x01]);
        assert_eq!(
            consume_value_payload(Marker::FixExt1, &mut reader, 0),
            Err(DecodeError::MessagePack),
        );
    }

    #[test]
    fn decodes_every_rns_1_4_2_operation() {
        let cases = [
            request(vec![("get", Value::from("interface_stats"))]),
            request(vec![
                ("get", Value::from("path_table")),
                ("max_hops", Value::Nil),
            ]),
            request(vec![("get", Value::from("rate_table"))]),
            request(vec![
                ("get", Value::from("next_hop_if_name")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![
                ("get", Value::from("next_hop")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![
                ("get", Value::from("first_hop_timeout")),
                ("destination_hash", binary::<16>(1)),
            ]),
            request(vec![("get", Value::from("link_count"))]),
            request(vec![
                ("get", Value::from("packet_rssi")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![
                ("get", Value::from("packet_snr")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![
                ("get", Value::from("packet_q")),
                ("packet_hash", binary::<16>(2)),
            ]),
            request(vec![("get", Value::from("blackholed_identities"))]),
            request(vec![
                ("get", Value::from("is_blackholed")),
                ("identity_hash", binary::<16>(3)),
            ]),
            request(vec![
                ("drop", Value::from("path")),
                ("destination_hash", binary::<16>(4)),
            ]),
            request(vec![
                ("drop", Value::from("all_via")),
                ("destination_hash", binary::<16>(4)),
            ]),
            request(vec![("drop", Value::from("announce_queues"))]),
            request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::Nil),
                ("reason", Value::Nil),
            ]),
            request(vec![("unblackhole_identity", binary::<16>(5))]),
            request(vec![
                ("destination_data", Value::from("used")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("destination_data", Value::from("retain")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("destination_data", Value::from("unretain")),
                ("destination_hash", binary::<16>(6)),
            ]),
            request(vec![
                ("identity_data", Value::from("retain")),
                ("identity_hash", binary::<16>(7)),
            ]),
        ];

        assert_eq!(cases.len(), 21);
        for bytes in cases {
            let decoded = decode(&bytes).unwrap();
            let encoded = decoded.encode_message_pack().unwrap();
            assert_eq!(decode(&encoded), Ok(decoded));
        }
    }

    #[test]
    fn preserves_numeric_and_optional_blackhole_arguments() {
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("path_table")),
                ("max_hops", Value::from(u64::MAX)),
            ])),
            Ok(RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::from_u64(u64::MAX)),
            })
        );
        assert_eq!(
            decode(&request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::F64(123.5)),
                ("reason", Value::from("operator request")),
            ])),
            Ok(RnsRpcRequest::BlackholeIdentity {
                identity_hash: IdentityHash::new([5; 16]),
                until: Some(RnsNumber::Float(123.5)),
                reason: Some("operator request".into()),
            })
        );
    }

    #[test]
    fn message_pack_numeric_arguments_round_trip_without_the_oracle_lane() {
        let identity_hash = IdentityHash::new([0x55; 16]);
        let cases = [
            RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::from_i64(-1)),
            },
            RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::from_u64(u64::MAX)),
            },
            RnsRpcRequest::BlackholeIdentity {
                identity_hash,
                until: Some(RnsNumber::Integer(RnsInteger::from_u64(u64::MAX))),
                reason: None,
            },
            RnsRpcRequest::BlackholeIdentity {
                identity_hash,
                until: Some(RnsNumber::Float(123.5)),
                reason: None,
            },
            RnsRpcRequest::BlackholeIdentity {
                identity_hash,
                until: None,
                reason: None,
            },
        ];
        for request in cases {
            let encoded = request.encode_message_pack().unwrap();
            assert_eq!(decode(&encoded), Ok(request));
        }
    }

    #[test]
    fn signed_integer_views_cover_negative_positive_and_overflow_boundaries() {
        assert_eq!(RnsInteger::from_i64(-1).signed_value(), Some(-1));
        assert_eq!(RnsInteger::from_i64(0).nonnegative_value(), Some(0));
        assert_eq!(RnsInteger::from_u64(0).signed_value(), Some(0));
        assert_eq!(
            RnsInteger::from_u64(i64::MAX as u64).signed_value(),
            Some(i64::MAX),
        );
        assert_eq!(
            RnsInteger::from_u64(i64::MAX as u64 + 1).signed_value(),
            None,
        );
        assert_eq!(RnsInteger::from_u64(u64::MAX).signed_value(), None);
    }

    #[test]
    fn blackhole_deadlines_preserve_rns_1_4_2_truthiness_and_epoch_seconds() {
        assert_eq!(
            RnsNumber::Integer(RnsInteger::from_i64(-1)).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(0))
        );
        assert_eq!(
            RnsNumber::Integer(RnsInteger::from_u64(0)).blackhole_expiry(),
            BlackholeExpiry::Indefinite
        );
        assert_eq!(
            RnsNumber::Float(f64::NAN).blackhole_expiry(),
            BlackholeExpiry::Indefinite
        );
        assert_eq!(
            RnsNumber::Float(f64::NEG_INFINITY).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(0)),
        );
        assert_eq!(
            RnsNumber::Float(f64::INFINITY).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(u64::MAX))
        );
        assert_eq!(
            RnsNumber::Float(123.4567).blackhole_expiry(),
            BlackholeExpiry::At(InstantMillis(123_456))
        );
    }

    #[test]
    fn packet_hash_arguments_preserve_the_rpc_lookup_key() {
        let truncated = decode(&request(vec![
            ("get", Value::from("packet_rssi")),
            ("packet_hash", binary::<16>(2)),
        ]));
        assert!(matches!(
            truncated,
            Ok(RnsRpcRequest::PacketRssi { packet_hash }) if packet_hash.packet_hash().is_none()
        ));

        let full = decode(&request(vec![
            ("get", Value::from("packet_rssi")),
            ("packet_hash", binary::<32>(2)),
        ]));
        assert!(matches!(
            full,
            Ok(RnsRpcRequest::PacketRssi { packet_hash })
                if packet_hash.packet_hash() == Some(PacketHash::new([2; PACKET_HASH_LEN]))
        ));
    }

    #[test]
    fn rejects_malformed_ambiguous_and_incomplete_requests() {
        assert_eq!(decode(&[]), Err(DecodeError::MessagePack));
        assert_eq!(decode(&[0xc0]), Err(DecodeError::ExpectedMap));
        assert_eq!(
            decode(&[
                0x81, 0xa6, b'r', b'e', b'a', b's', b'o', b'n', 0xc6, 0xff, 0xff, 0xff, 0xff,
            ]),
            Err(DecodeError::MessagePack)
        );

        let mut trailing = request(vec![("get", Value::from("link_count"))]);
        trailing.push(0xc0);
        assert_eq!(decode(&trailing), Err(DecodeError::TrailingData));

        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("drop", Value::from("announce_queues")),
            ])),
            Err(DecodeError::ContradictoryOperation)
        );
        assert_eq!(
            decode(&request(vec![("destination_hash", binary::<16>(1))])),
            Err(DecodeError::MissingOperation)
        );
        assert_eq!(
            decode(&request(vec![("get", Value::from("next_hop"))])),
            Err(DecodeError::MissingField("destination_hash"))
        );
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("next_hop")),
                ("destination_hash", binary::<15>(1)),
            ])),
            Err(DecodeError::InvalidHashLength {
                field: "destination_hash",
                expected: 16,
                actual: 15,
            })
        );
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("reason", Value::from("interface_stats")),
            ])),
            Err(DecodeError::UnexpectedField("reason"))
        );
    }

    #[test]
    fn rejects_duplicate_and_unknown_fields() {
        assert_eq!(
            decode(&request(vec![
                ("get", Value::from("link_count")),
                ("get", Value::from("rate_table")),
            ])),
            Err(DecodeError::DuplicateField("get".into()))
        );
        assert_eq!(
            decode(&request(vec![("future", Value::from("link_count"))])),
            Err(DecodeError::UnknownField("future".into()))
        );
    }

    #[test]
    fn signed_positive_integer_markers_remain_nonnegative() {
        let bytes = [
            0x82, 0xa3, b'g', b'e', b't', 0xaa, b'p', b'a', b't', b'h', b'_', b't', b'a', b'b',
            b'l', b'e', 0xa8, b'm', b'a', b'x', b'_', b'h', b'o', b'p', b's', 0xd0, 0x03,
        ];
        assert_eq!(
            decode(&bytes),
            Ok(RnsRpcRequest::PathTable {
                max_hops: Some(RnsInteger::from_u64(3)),
            })
        );
    }

    #[test]
    fn nesting_beyond_the_protocol_limit_is_rejected() {
        let encoded = |value: &Value| {
            let mut bytes = Vec::new();
            rmpv::encode::write_value(&mut bytes, value).unwrap();
            bytes
        };
        for maps in [false, true] {
            let nested = |levels: usize| {
                let mut value = Value::Boolean(true);
                for _ in 0..levels {
                    value = if maps {
                        Value::Map(vec![(Value::Nil, value)])
                    } else {
                        Value::Array(vec![value])
                    };
                }
                value
            };
            let accepted = encoded(&nested(REQUEST_MAX_DEPTH));
            let mut reader = Bytes::new(&accepted);
            assert!(decode_value(&mut reader, 0).is_ok());
            assert!(reader.remaining_slice().is_empty());

            let rejected = encoded(&nested(REQUEST_MAX_DEPTH + 1));
            let mut reader = Bytes::new(&rejected);
            assert!(matches!(
                decode_value(&mut reader, 0),
                Err(DecodeError::MessagePack),
            ));
        }

        let mut nested_key = Value::Nil;
        for _ in 0..REQUEST_MAX_DEPTH {
            nested_key = Value::Array(vec![nested_key]);
        }
        let keyed_map = encoded(&Value::Map(vec![(nested_key, Value::Nil)]));
        let mut reader = Bytes::new(&keyed_map);
        assert!(matches!(
            decode_value(&mut reader, 0),
            Err(DecodeError::MessagePack)
        ));

        let mut nested = Value::Nil;
        for _ in 0..=REQUEST_MAX_DEPTH {
            nested = Value::Array(vec![nested]);
        }
        assert_eq!(
            decode(&request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::Nil),
                ("reason", nested),
            ])),
            Err(DecodeError::MessagePack)
        );

        let mut nested = Value::Nil;
        for _ in 0..=REQUEST_MAX_DEPTH {
            nested = Value::Map(vec![(Value::Nil, nested)]);
        }
        assert_eq!(
            decode(&request(vec![
                ("blackhole_identity", binary::<16>(5)),
                ("until", Value::Nil),
                ("reason", nested),
            ])),
            Err(DecodeError::MessagePack)
        );
    }

    #[test]
    fn generated_payload_corpus_has_total_decode_results() {
        for seed in 0u16..256 {
            let mut state = u32::from(seed).wrapping_add(1);
            let bytes = (0..usize::from(seed % 128))
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    state.to_be_bytes()[0]
                })
                .collect::<Vec<_>>();
            let _ = decode(&bytes);
        }
    }
}
