//! Allocation-free LXMF wire parsing and cryptographic input construction.
//!
//! This crate owns the transport-neutral boundary between complete LXMF bytes
//! and the carriers which deliver them. Parsed messages borrow their input,
//! and every MessagePack walk is constrained by caller-provided [`WireLimits`].

#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use core::fmt;

use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

mod announce;
mod composer;
mod msgpack;
mod stamp;

pub use announce::{
    encode_current_lxmf_announce, parse_lxmf_announce, AnnounceError, AnnounceFormat, LxmfAnnounce,
    LXMF_APP_NAME, LXMF_DELIVERY_ASPECTS, LXMF_DELIVERY_DOTTED_NAME_HASH,
};
pub use composer::{
    compose_basic_direct_lxmf, BasicLxmfComposeError, BasicLxmfSigner, PreparedBasicLxmf,
    MAX_BASIC_LXMF_WIRE_BYTES, MAX_LXMF_TIMESTAMP_UNIX_MS,
};
pub use msgpack::{validate_messagepack_value, Canonicality, MessagePackKind, MessagePackValue};
pub use stamp::{
    validate_pow_stamp, validate_ticket_stamp, PowBudget, PowStampValidation, StampError,
    POW_EXPAND_ROUNDS,
};

/// Length of an RNS destination hash used by LXMF.
pub const DESTINATION_HASH_LENGTH: usize = 16;
/// Length of an RNS identity public key (X25519 followed by Ed25519).
pub const IDENTITY_PUBLIC_KEY_LENGTH: usize = 64;
/// Length of an LXMF Ed25519 signature.
pub const SIGNATURE_LENGTH: usize = 64;
/// Length of a complete LXMF prefix before its MessagePack payload.
pub const WIRE_HEADER_LENGTH: usize = DESTINATION_HASH_LENGTH * 2 + SIGNATURE_LENGTH;
/// Retained source-hash and signature prefix in opportunistic carrier data.
pub const OPPORTUNISTIC_HEADER_LENGTH: usize = DESTINATION_HASH_LENGTH + SIGNATURE_LENGTH;
/// Length of an LXMF message ID.
pub const MESSAGE_ID_LENGTH: usize = 32;
/// Length of the domain-separated authenticated-material fingerprint used by
/// durable stores to distinguish a replay from a theoretical message-ID
/// collision.
pub const AUTHENTICATED_MATERIAL_FINGERPRINT_LENGTH: usize = 32;
/// Length of an LXMF proof-of-work stamp.
pub const POW_STAMP_LENGTH: usize = 32;
/// Length of an LXMF ticket and ticket-derived stamp.
pub const TICKET_LENGTH: usize = 16;
/// Largest encrypted opportunistic content size selected by pinned Python LXMF 1.1.0.
pub const ENCRYPTED_PACKET_MAX_CONTENT: usize = 295;
/// Largest direct Link packet content size selected by pinned Python LXMF 1.1.0.
pub const LINK_PACKET_MAX_CONTENT: usize = 319;
/// Bytes subtracted from an encoded basic LXMF payload when selecting its
/// opportunistic or direct delivery lane.
///
/// This is Python LXMF's destination hash and signature accounting term. It is
/// intentionally separate from the complete normalized wire header length.
pub const BASIC_LXMF_SELECTION_OVERHEAD_BYTES: usize = 16;
/// Encoded MessagePack length of an empty LXMF fields map.
pub const EMPTY_LXMF_FIELDS_ENCODED_BYTES: usize = 1;
/// Hard recursion bound independent of a board profile's configured limit.
pub const ABSOLUTE_MAX_NESTING_DEPTH: usize = 32;

/// Return the encoded MessagePack size of one binary value.
///
/// `None` means the input exceeds MessagePack's unsigned 32-bit binary length.
#[must_use]
pub fn encoded_messagepack_binary_len(value_bytes: usize) -> Option<usize> {
    let header_bytes: usize = if value_bytes <= u8::MAX as usize {
        2
    } else if value_bytes <= u16::MAX as usize {
        3
    } else if value_bytes <= u32::MAX as usize {
        5
    } else {
        return None;
    };
    header_bytes.checked_add(value_bytes)
}

/// Return the encoded four-item basic LXMF payload length.
///
/// `fields_encoded_bytes` is the complete encoded MessagePack fields map, not
/// only its contents. The fixed ten bytes are the array marker and float64
/// timestamp. `None` reports arithmetic or MessagePack-length overflow.
#[must_use]
pub fn encoded_basic_lxmf_payload_len(
    title_bytes: usize,
    content_bytes: usize,
    fields_encoded_bytes: usize,
) -> Option<usize> {
    10_usize
        .checked_add(encoded_messagepack_binary_len(title_bytes)?)?
        .checked_add(encoded_messagepack_binary_len(content_bytes)?)?
        .checked_add(fields_encoded_bytes)
}

/// Whether one basic LXMF payload fits a delivery lane's content ceiling.
///
/// This mirrors LXMF's signed `payload_len - 16` selection comparison without
/// unsigned underflow for the canonical empty payload.
#[must_use]
pub fn basic_lxmf_payload_fits_content_limit(
    title_bytes: usize,
    content_bytes: usize,
    fields_encoded_bytes: usize,
    maximum_content_bytes: usize,
) -> bool {
    let Some(payload_len) =
        encoded_basic_lxmf_payload_len(title_bytes, content_bytes, fields_encoded_bytes)
    else {
        return false;
    };
    let Some(maximum_payload_len) =
        BASIC_LXMF_SELECTION_OVERHEAD_BYTES.checked_add(maximum_content_bytes)
    else {
        return false;
    };
    payload_len <= maximum_payload_len
}

const PAYLOAD_ARRAY_4: u8 = 0x94;
const PAYLOAD_ARRAY_5: u8 = 0x95;
const MSGPACK_F64: u8 = 0xcb;
const PYTHON_STRUCT_AND_TIMESTAMP_SIZE: usize = 16;
const LXMF_DELIVERY_NAME_HASH: [u8; 10] =
    [0x6e, 0xc6, 0x0b, 0xc3, 0x18, 0xe2, 0xc0, 0xf0, 0xd9, 0x08];
const AUTHENTICATED_MATERIAL_FINGERPRINT_DOMAIN: &[u8] =
    b"reticulum-rs-firmware/lxmf/authenticated-material/v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdwardsPointError {
    InvalidEncoding,
    NonPrimeOrder,
}

fn require_non_identity_prime_order_point(encoded: &[u8; 32]) -> Result<(), EdwardsPointError> {
    let point = CompressedEdwardsY(*encoded)
        .decompress()
        .ok_or(EdwardsPointError::InvalidEncoding)?;
    if point.is_small_order() || !point.is_torsion_free() {
        Err(EdwardsPointError::NonPrimeOrder)
    } else {
        Ok(())
    }
}

/// Resource ceilings applied while admitting one LXMF message.
///
/// There is deliberately no implicit default. A board profile chooses these
/// limits according to its RAM, storage and trust boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireLimits {
    /// Maximum normalized complete wire length, including the implied hash for
    /// opportunistic ingress.
    pub max_wire_bytes: usize,
    /// Maximum payload bytes in one MessagePack string, binary or extension.
    pub max_value_bytes: usize,
    /// Maximum entries in one map or items in one array.
    pub max_container_items: usize,
    /// Maximum total MessagePack values visited, including container roots.
    pub max_total_values: usize,
    /// Maximum scanner steps, including bounded duplicate-key comparison rescans.
    pub max_scan_steps: usize,
    /// Maximum MessagePack nesting depth, with the LXMF payload array at depth one.
    pub max_nesting_depth: usize,
}

impl WireLimits {
    /// Construct explicit wire limits.
    pub const fn new(
        max_wire_bytes: usize,
        max_value_bytes: usize,
        max_container_items: usize,
        max_total_values: usize,
        max_scan_steps: usize,
        max_nesting_depth: usize,
    ) -> Self {
        Self {
            max_wire_bytes,
            max_value_bytes,
            max_container_items,
            max_total_values,
            max_scan_steps,
            max_nesting_depth,
        }
    }
}

/// Which bounded MessagePack resource was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Normalized complete LXMF bytes.
    WireBytes,
    /// Bytes in one MessagePack string, binary or extension payload.
    ValueBytes,
    /// Items or entries in one MessagePack container.
    ContainerItems,
    /// Values visited across the entire payload.
    TotalValues,
    /// Structural scan operations, including map-key comparison rescans.
    ScanSteps,
    /// Nested MessagePack value depth.
    NestingDepth,
}

/// A malformed or over-budget LXMF wire message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// The fixed LXMF prefix or payload is incomplete.
    TooShort { minimum: usize, actual: usize },
    /// A MessagePack object ended before all declared bytes were available.
    Truncated {
        offset: usize,
        needed: usize,
        available: usize,
    },
    /// MessagePack length arithmetic overflowed the target architecture.
    LengthOverflow { offset: usize },
    /// The reserved MessagePack marker `0xc1` was encountered.
    ReservedMarker { offset: usize },
    /// A MessagePack string did not contain valid UTF-8.
    InvalidUtf8 { offset: usize },
    /// MessagePack extension type `-1` requires Python datetime normalization
    /// outside this wire model.
    UnsupportedTimestampExtension { offset: usize },
    /// A caller-owned resource ceiling was exceeded.
    LimitExceeded {
        kind: LimitKind,
        actual: usize,
        maximum: usize,
        offset: usize,
    },
    /// More bytes followed the one complete MessagePack object.
    TrailingBytes { offset: usize, trailing: usize },
    /// Explicit direct/resource destination does not match the local carrier destination.
    DestinationMismatch,
    /// LXMF payload was not an array containing exactly four or five values.
    InvalidPayloadArray { marker: u8, count: Option<u32> },
    /// LXMF timestamp was not a MessagePack float64.
    InvalidTimestamp { marker: u8 },
    /// LXMF title was not MessagePack binary data.
    InvalidTitle { marker: u8 },
    /// LXMF content was not MessagePack binary data.
    InvalidContent { marker: u8 },
    /// LXMF fields root was not a MessagePack map.
    InvalidFields { marker: u8 },
    /// LXMF stamp was not binary data of a protocol-defined length.
    InvalidStamp { marker: u8, length: usize },
    /// A stamped payload was not a fixed point under Python u-msgpack re-encoding.
    NonCanonicalStampedPayload,
    /// A map key cannot be proven hashable with Python-compatible equality.
    UnsupportedMapKey { offset: usize },
    /// A map contains keys equal under Python semantics.
    DuplicateMapKey { offset: usize },
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { minimum, actual } => {
                write!(
                    formatter,
                    "LXMF wire is {actual} bytes; at least {minimum} are required"
                )
            }
            Self::Truncated {
                offset,
                needed,
                available,
            } => write!(
                formatter,
                "MessagePack value at offset {offset} needs {needed} bytes; {available} remain"
            ),
            Self::LengthOverflow { offset } => {
                write!(
                    formatter,
                    "MessagePack length at offset {offset} overflows usize"
                )
            }
            Self::ReservedMarker { offset } => {
                write!(formatter, "reserved MessagePack marker at offset {offset}")
            }
            Self::InvalidUtf8 { offset } => {
                write!(
                    formatter,
                    "invalid MessagePack UTF-8 string at offset {offset}"
                )
            }
            Self::UnsupportedTimestampExtension { offset } => write!(
                formatter,
                "MessagePack timestamp extension at offset {offset} is unsupported"
            ),
            Self::LimitExceeded {
                kind,
                actual,
                maximum,
                offset,
            } => write!(
                formatter,
                "{kind:?} limit exceeded at offset {offset}: {actual} > {maximum}"
            ),
            Self::TrailingBytes { offset, trailing } => {
                write!(formatter, "{trailing} trailing bytes after offset {offset}")
            }
            Self::DestinationMismatch => {
                formatter.write_str("LXMF destination does not match local carrier destination")
            }
            Self::InvalidPayloadArray { marker, count } => {
                write!(
                    formatter,
                    "invalid LXMF payload array marker 0x{marker:02x} or count {count:?}"
                )
            }
            Self::InvalidTimestamp { marker } => {
                write!(formatter, "invalid LXMF timestamp marker 0x{marker:02x}")
            }
            Self::InvalidTitle { marker } => {
                write!(formatter, "invalid LXMF title marker 0x{marker:02x}")
            }
            Self::InvalidContent { marker } => {
                write!(formatter, "invalid LXMF content marker 0x{marker:02x}")
            }
            Self::InvalidFields { marker } => {
                write!(formatter, "invalid LXMF fields marker 0x{marker:02x}")
            }
            Self::InvalidStamp { marker, length } => write!(
                formatter,
                "invalid LXMF stamp marker 0x{marker:02x} or length {length}"
            ),
            Self::NonCanonicalStampedPayload => formatter.write_str(
                "stamped LXMF payload is not a fixed point under Python u-msgpack encoding",
            ),
            Self::UnsupportedMapKey { offset } => {
                write!(
                    formatter,
                    "unsupported MessagePack map key at offset {offset}"
                )
            }
            Self::DuplicateMapKey { offset } => {
                write!(
                    formatter,
                    "duplicate MessagePack map key at offset {offset}"
                )
            }
        }
    }
}

/// One binary LXMF title or content view.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BinaryView<'a> {
    bytes: &'a [u8],
    encoded_len: usize,
}

impl<'a> BinaryView<'a> {
    /// Decoded binary bytes.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Encoded MessagePack length, including its marker and length prefix.
    pub const fn encoded_len(self) -> usize {
        self.encoded_len
    }
}

impl fmt::Debug for BinaryView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BinaryView")
            .field("decoded_len", &self.bytes.len())
            .field("encoded_len", &self.encoded_len)
            .finish()
    }
}

/// Borrowed, structurally validated LXMF payload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PayloadView<'a> {
    raw: &'a [u8],
    first_four: [&'a [u8]; 4],
    timestamp_bits: u64,
    title: BinaryView<'a>,
    content: BinaryView<'a>,
    fields: MessagePackValue<'a>,
    stamp: Option<BinaryView<'a>>,
}

impl<'a> PayloadView<'a> {
    /// Original four- or five-item MessagePack payload bytes.
    pub const fn raw(self) -> &'a [u8] {
        self.raw
    }

    /// Exact IEEE-754 bits from the MessagePack float64 timestamp.
    pub const fn timestamp_bits(self) -> u64 {
        self.timestamp_bits
    }

    /// Timestamp as an IEEE-754 value.
    pub fn timestamp(self) -> f64 {
        f64::from_bits(self.timestamp_bits)
    }

    /// Binary title bytes.
    pub const fn title(self) -> BinaryView<'a> {
        self.title
    }

    /// Binary content bytes.
    pub const fn content(self) -> BinaryView<'a> {
        self.content
    }

    /// Raw, structurally validated MessagePack fields map.
    pub const fn fields(self) -> MessagePackValue<'a> {
        self.fields
    }

    /// Optional 16-byte ticket or 32-byte proof-of-work stamp.
    pub const fn stamp(self) -> Option<BinaryView<'a>> {
        self.stamp
    }

    /// Canonical four-item payload length used in message IDs and signatures.
    pub fn payload_without_stamp_len(self) -> usize {
        if self.stamp.is_none() {
            self.raw.len()
        } else {
            1 + self.first_four.iter().map(|item| item.len()).sum::<usize>()
        }
    }

    /// Write the exact Python-compatible four-item payload representation.
    pub fn write_payload_without_stamp(self, output: &mut [u8]) -> Result<usize, BufferError> {
        let required = self.payload_without_stamp_len();
        if output.len() < required {
            return Err(BufferError {
                required,
                available: output.len(),
            });
        }
        if self.stamp.is_none() {
            output[..required].copy_from_slice(self.raw);
            return Ok(required);
        }

        output[0] = PAYLOAD_ARRAY_4;
        let mut cursor = 1;
        for item in self.first_four {
            let end = cursor + item.len();
            output[cursor..end].copy_from_slice(item);
            cursor = end;
        }
        Ok(cursor)
    }
}

impl fmt::Debug for PayloadView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PayloadView")
            .field("encoded_len", &self.raw.len())
            .field("timestamp_type", &"f64")
            .field("title_len", &self.title.bytes.len())
            .field("content_len", &self.content.bytes.len())
            .field("fields", &self.fields)
            .field("stamp_len", &self.stamp.map(|stamp| stamp.bytes.len()))
            .finish()
    }
}

/// Carrier form from which an LXMF message arrived.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CarrierIngress<'a> {
    /// Destination DATA omits the destination hash; the RNS destination supplies it.
    Opportunistic {
        implied_destination: &'a [u8; DESTINATION_HASH_LENGTH],
        payload: &'a [u8],
    },
    /// Direct `LinkData` already proven to have RNS context `NONE` carries complete LXMF bytes.
    LinkDataContextNone {
        /// Local LXMF destination on the established link.
        expected_destination: &'a [u8; DESTINATION_HASH_LENGTH],
        /// Complete LXMF bytes.
        payload: &'a [u8],
    },
    /// A completed RNS Resource carries complete LXMF bytes.
    ResourceComplete {
        /// Local LXMF destination on the resource's established link.
        expected_destination: &'a [u8; DESTINATION_HASH_LENGTH],
        /// Complete LXMF bytes.
        payload: &'a [u8],
    },
}

impl fmt::Debug for CarrierIngress<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opportunistic { payload, .. } => formatter
                .debug_struct("Opportunistic")
                .field("payload_len", &payload.len())
                .finish(),
            Self::LinkDataContextNone { payload, .. } => formatter
                .debug_struct("LinkDataContextNone")
                .field("payload_len", &payload.len())
                .finish(),
            Self::ResourceComplete { payload, .. } => formatter
                .debug_struct("ResourceComplete")
                .field("payload_len", &payload.len())
                .finish(),
        }
    }
}

/// Carrier provenance retained after LXMF normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierKind {
    /// Complete bytes parsed without stronger carrier provenance.
    Complete,
    /// Destination DATA with an implied destination hash.
    Opportunistic,
    /// Link data whose RNS context was already checked as `NONE`.
    LinkDataContextNone,
    /// Completed RNS Resource.
    ResourceComplete,
}

/// Borrowed normalized LXMF message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MessageView<'a> {
    destination_hash: &'a [u8; DESTINATION_HASH_LENGTH],
    source_hash: &'a [u8; DESTINATION_HASH_LENGTH],
    signature: &'a [u8; SIGNATURE_LENGTH],
    body: &'a [u8],
    payload: PayloadView<'a>,
    normalized_wire_len: usize,
    carrier: CarrierKind,
    destination_bound: bool,
}

impl<'a> MessageView<'a> {
    /// Parse complete LXMF bytes containing destination, source and signature.
    pub fn parse_complete(wire: &'a [u8], limits: WireLimits) -> Result<Self, WireError> {
        if wire.len() < WIRE_HEADER_LENGTH + 1 {
            return Err(WireError::TooShort {
                minimum: WIRE_HEADER_LENGTH + 1,
                actual: wire.len(),
            });
        }
        enforce_wire_limit(wire.len(), limits)?;
        let destination_hash: &[u8; DESTINATION_HASH_LENGTH] = wire[..DESTINATION_HASH_LENGTH]
            .try_into()
            .map_err(|_| WireError::TooShort {
                minimum: WIRE_HEADER_LENGTH + 1,
                actual: wire.len(),
            })?;
        Self::parse_parts(
            destination_hash,
            &wire[DESTINATION_HASH_LENGTH..],
            wire.len(),
            CarrierKind::Complete,
            false,
            limits,
        )
    }

    /// Parse an explicit carrier form, normalizing opportunistic destination omission.
    pub fn parse_ingress(
        ingress: CarrierIngress<'a>,
        limits: WireLimits,
    ) -> Result<Self, WireError> {
        match ingress {
            CarrierIngress::Opportunistic {
                implied_destination,
                payload,
            } => {
                let normalized_wire_len = payload
                    .len()
                    .checked_add(DESTINATION_HASH_LENGTH)
                    .ok_or(WireError::LengthOverflow { offset: 0 })?;
                if payload.len() < OPPORTUNISTIC_HEADER_LENGTH + 1 {
                    return Err(WireError::TooShort {
                        minimum: OPPORTUNISTIC_HEADER_LENGTH + 1,
                        actual: payload.len(),
                    });
                }
                enforce_wire_limit(normalized_wire_len, limits)?;
                Self::parse_parts(
                    implied_destination,
                    payload,
                    normalized_wire_len,
                    CarrierKind::Opportunistic,
                    true,
                    limits,
                )
            }
            CarrierIngress::LinkDataContextNone {
                expected_destination,
                payload,
            } => Self::parse_complete_with_carrier(
                payload,
                expected_destination,
                CarrierKind::LinkDataContextNone,
                limits,
            ),
            CarrierIngress::ResourceComplete {
                expected_destination,
                payload,
            } => Self::parse_complete_with_carrier(
                payload,
                expected_destination,
                CarrierKind::ResourceComplete,
                limits,
            ),
        }
    }

    fn parse_complete_with_carrier(
        wire: &'a [u8],
        expected_destination: &[u8; DESTINATION_HASH_LENGTH],
        carrier: CarrierKind,
        limits: WireLimits,
    ) -> Result<Self, WireError> {
        if wire.len() < WIRE_HEADER_LENGTH + 1 {
            return Err(WireError::TooShort {
                minimum: WIRE_HEADER_LENGTH + 1,
                actual: wire.len(),
            });
        }
        enforce_wire_limit(wire.len(), limits)?;
        let destination_hash: &[u8; DESTINATION_HASH_LENGTH] = wire[..DESTINATION_HASH_LENGTH]
            .try_into()
            .map_err(|_| WireError::TooShort {
                minimum: WIRE_HEADER_LENGTH + 1,
                actual: wire.len(),
            })?;
        if destination_hash.ct_eq(expected_destination).unwrap_u8() != 1 {
            return Err(WireError::DestinationMismatch);
        }
        let message = Self::parse_parts(
            destination_hash,
            &wire[DESTINATION_HASH_LENGTH..],
            wire.len(),
            carrier,
            true,
            limits,
        )?;
        Ok(message)
    }

    fn parse_parts(
        destination_hash: &'a [u8; DESTINATION_HASH_LENGTH],
        body: &'a [u8],
        normalized_wire_len: usize,
        carrier: CarrierKind,
        destination_bound: bool,
        limits: WireLimits,
    ) -> Result<Self, WireError> {
        let source_hash =
            body[..DESTINATION_HASH_LENGTH]
                .try_into()
                .map_err(|_| WireError::TooShort {
                    minimum: OPPORTUNISTIC_HEADER_LENGTH + 1,
                    actual: body.len(),
                })?;
        let signature = body[DESTINATION_HASH_LENGTH..OPPORTUNISTIC_HEADER_LENGTH]
            .try_into()
            .map_err(|_| WireError::TooShort {
                minimum: OPPORTUNISTIC_HEADER_LENGTH + 1,
                actual: body.len(),
            })?;
        let raw_payload = &body[OPPORTUNISTIC_HEADER_LENGTH..];
        let payload = parse_payload(raw_payload, limits)?;
        Ok(Self {
            destination_hash,
            source_hash,
            signature,
            body,
            payload,
            normalized_wire_len,
            carrier,
            destination_bound,
        })
    }

    /// Destination hash, whether explicit on the carrier or supplied by RNS.
    pub const fn destination_hash(self) -> &'a [u8; DESTINATION_HASH_LENGTH] {
        self.destination_hash
    }

    /// Source `lxmf.delivery` destination hash.
    pub const fn source_hash(self) -> &'a [u8; DESTINATION_HASH_LENGTH] {
        self.source_hash
    }

    /// Ed25519 signature bytes.
    pub const fn signature(self) -> &'a [u8; SIGNATURE_LENGTH] {
        self.signature
    }

    /// Contiguous carrier body beginning at the source hash.
    pub const fn carrier_body(self) -> &'a [u8] {
        self.body
    }

    /// Parsed LXMF payload.
    pub const fn payload(self) -> PayloadView<'a> {
        self.payload
    }

    /// Complete normalized length, including an opportunistically implied destination hash.
    pub const fn normalized_wire_len(self) -> usize {
        self.normalized_wire_len
    }

    /// Carrier provenance captured during normalization.
    pub const fn carrier_kind(self) -> CarrierKind {
        self.carrier
    }

    /// Whether the carrier proved this message belongs to its expected local destination.
    pub const fn is_destination_bound(self) -> bool {
        self.destination_bound
    }

    /// Python LXMF's content-size selector input, saturated at zero for a
    /// structurally valid empty payload whose historical formula is negative.
    pub fn selection_content_size(self) -> usize {
        self.payload
            .raw
            .len()
            .saturating_sub(PYTHON_STRUCT_AND_TIMESTAMP_SIZE)
    }

    /// Calculate `SHA-256(destination || source || payload_without_stamp)`.
    pub fn message_id(self) -> [u8; MESSAGE_ID_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(self.destination_hash);
        hasher.update(self.source_hash);
        self.update_payload_without_stamp(&mut hasher);
        hasher.finalize().into()
    }

    /// Calculate a domain-separated digest of the exact authenticated LXMF
    /// hash material.
    ///
    /// Unlike [`Self::message_id`], this value is not part of the LXMF
    /// protocol. It gives a durable store an independent replay/collision
    /// discriminator while preserving Python's exact four-item raw and
    /// stamped canonicalization rules. A different valid stamp therefore does
    /// not change this fingerprint.
    pub fn authenticated_material_fingerprint(
        self,
    ) -> [u8; AUTHENTICATED_MATERIAL_FINGERPRINT_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(AUTHENTICATED_MATERIAL_FINGERPRINT_DOMAIN);
        hasher.update(self.destination_hash);
        hasher.update(self.source_hash);
        self.update_payload_without_stamp(&mut hasher);
        hasher.finalize().into()
    }

    /// Bytes required by [`Self::write_signature_input`].
    pub fn signature_input_len(self) -> usize {
        DESTINATION_HASH_LENGTH * 2 + self.payload.payload_without_stamp_len() + MESSAGE_ID_LENGTH
    }

    /// Write `destination || source || payload_without_stamp || message_id`.
    pub fn write_signature_input(self, output: &mut [u8]) -> Result<usize, BufferError> {
        let required = self.signature_input_len();
        if output.len() < required {
            return Err(BufferError {
                required,
                available: output.len(),
            });
        }
        let mut cursor = 0;
        copy_part(output, &mut cursor, self.destination_hash);
        copy_part(output, &mut cursor, self.source_hash);
        if self.payload.stamp.is_none() {
            copy_part(output, &mut cursor, self.payload.raw);
        } else {
            output[cursor] = PAYLOAD_ARRAY_4;
            cursor += 1;
            for item in self.payload.first_four {
                copy_part(output, &mut cursor, item);
            }
        }
        copy_part(output, &mut cursor, &self.message_id());
        Ok(cursor)
    }

    /// Bind an announced 64-byte RNS public key to this message's
    /// `lxmf.delivery` source hash before it can be used for verification.
    pub fn bind_source_identity<'key>(
        self,
        public_key: &'key [u8; IDENTITY_PUBLIC_KEY_LENGTH],
    ) -> Result<BoundSourceIdentity<'key>, SignatureError> {
        BoundSourceIdentity::new(public_key, self.source_hash)
    }

    /// Verify the LXMF signature with a source key already bound to this exact
    /// `lxmf.delivery` destination hash.
    pub fn verify_signature(
        self,
        source: &BoundSourceIdentity<'_>,
    ) -> Result<SignatureVerifiedMessage<'a>, SignatureError> {
        if source.destination_hash.ct_eq(self.source_hash).unwrap_u8() != 1 {
            return Err(SignatureError::SourceHashMismatch);
        }
        let signature = Signature::from_bytes(self.signature);
        let signature_point_bytes: &[u8; 32] = self.signature[..32]
            .try_into()
            .map_err(|_| SignatureError::InvalidSignaturePoint)?;
        match require_non_identity_prime_order_point(signature_point_bytes) {
            Ok(()) => {}
            Err(EdwardsPointError::InvalidEncoding) => {
                return Err(SignatureError::InvalidSignaturePoint);
            }
            Err(EdwardsPointError::NonPrimeOrder) => {
                return Err(SignatureError::NonPrimeOrderSignaturePoint);
            }
        }
        let mut verifier = source
            .verifying_key
            .verify_stream(&signature)
            .map_err(|_| SignatureError::InvalidSignature)?;
        verifier.update(self.destination_hash);
        verifier.update(self.source_hash);
        if self.payload.stamp.is_none() {
            verifier.update(self.payload.raw);
        } else {
            verifier.update([PAYLOAD_ARRAY_4]);
            for item in self.payload.first_four {
                verifier.update(item);
            }
        }
        verifier.update(self.message_id());
        verifier
            .finalize_and_verify()
            .map_err(|_| SignatureError::InvalidSignature)?;
        Ok(SignatureVerifiedMessage { message: self })
    }

    fn update_payload_without_stamp(self, hasher: &mut Sha256) {
        if self.payload.stamp.is_none() {
            hasher.update(self.payload.raw);
        } else {
            hasher.update([PAYLOAD_ARRAY_4]);
            for item in self.payload.first_four {
                hasher.update(item);
            }
        }
    }
}

impl fmt::Debug for MessageView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessageView")
            .field("normalized_wire_len", &self.normalized_wire_len)
            .field("carrier", &self.carrier)
            .field("destination_bound", &self.destination_bound)
            .field("carrier_body_len", &self.body.len())
            .field("destination_hash_len", &self.destination_hash.len())
            .field("source_hash_len", &self.source_hash.len())
            .field("signature_len", &self.signature.len())
            .field("payload", &self.payload)
            .finish()
    }
}

/// LXMF message whose source hash binding and Ed25519 signature were verified.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SignatureVerifiedMessage<'a> {
    message: MessageView<'a>,
}

impl<'a> SignatureVerifiedMessage<'a> {
    /// Authenticated borrowed message.
    pub const fn message(self) -> MessageView<'a> {
        self.message
    }

    /// Apply one explicit receiver-owned stamp policy and cross the final
    /// authenticated local-admission boundary.
    pub fn apply_stamp_policy(
        self,
        policy: StampPolicy<'_>,
    ) -> Result<ValidatedMessage<'a>, StampPolicyError> {
        if !self.message.destination_bound {
            return Err(StampPolicyError::DestinationUnbound);
        }
        let stamp = self.message.payload.stamp.map(BinaryView::as_bytes);
        let message_id = self.message.message_id();
        let admission = match policy {
            StampPolicy::NotRequired => StampAdmission::NotRequired {
                stamp_present: stamp.is_some(),
            },
            StampPolicy::TrustedPriorTickets(tickets) => {
                let stamp = require_stamp_kind(stamp, StampKind::Ticket)?;
                if !any_ticket_valid(stamp, &message_id, tickets) {
                    return Err(StampPolicyError::Rejected {
                        kind: StampKind::Ticket,
                    });
                }
                StampAdmission::TrustedPriorTicket
            }
            StampPolicy::ProofOfWork {
                target_cost,
                budget,
            } => {
                let stamp = require_stamp_kind(stamp, StampKind::ProofOfWork)?;
                let result = validate_pow_stamp(stamp, &message_id, target_cost.get(), budget)
                    .map_err(StampPolicyError::Validation)?;
                if !result.valid {
                    return Err(StampPolicyError::Rejected {
                        kind: StampKind::ProofOfWork,
                    });
                }
                StampAdmission::ProofOfWork {
                    target_cost,
                    value: result.value,
                }
            }
            StampPolicy::TrustedPriorTicketsOrProofOfWork {
                tickets,
                target_cost,
                budget,
            } => match stamp_kind(stamp)? {
                (StampKind::Ticket, stamp) => {
                    if !any_ticket_valid(stamp, &message_id, tickets) {
                        return Err(StampPolicyError::Rejected {
                            kind: StampKind::Ticket,
                        });
                    }
                    StampAdmission::TrustedPriorTicket
                }
                (StampKind::ProofOfWork, stamp) => {
                    let result = validate_pow_stamp(stamp, &message_id, target_cost.get(), budget)
                        .map_err(StampPolicyError::Validation)?;
                    if !result.valid {
                        return Err(StampPolicyError::Rejected {
                            kind: StampKind::ProofOfWork,
                        });
                    }
                    StampAdmission::ProofOfWork {
                        target_cost,
                        value: result.value,
                    }
                }
            },
        };
        Ok(ValidatedMessage {
            verified: self,
            admission,
        })
    }
}

impl fmt::Debug for SignatureVerifiedMessage<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignatureVerifiedMessage")
            .field("message", &self.message)
            .finish()
    }
}

/// Receiver-owned stamp acceptance rule.
pub enum StampPolicy<'a> {
    /// Stamp validation is explicitly not required for this destination policy.
    NotRequired,
    /// Require a ticket stamp matching one previously issued trusted ticket.
    TrustedPriorTickets(&'a [[u8; TICKET_LENGTH]]),
    /// Require a regular LXMF proof-of-work stamp.
    ProofOfWork {
        /// Receiver-required cost.
        target_cost: RequiredStampCost,
        /// Explicit CPU authorization.
        budget: PowBudget,
    },
    /// Accept either a prior trusted ticket or proof of work according to stamp width.
    TrustedPriorTicketsOrProofOfWork {
        /// Previously issued receiver-owned tickets; never the current message's fields.
        tickets: &'a [[u8; TICKET_LENGTH]],
        /// Receiver-required proof-of-work cost.
        target_cost: RequiredStampCost,
        /// Explicit CPU authorization used only for a 32-byte stamp.
        budget: PowBudget,
    },
}

impl fmt::Debug for StampPolicy<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRequired => formatter.write_str("NotRequired"),
            Self::TrustedPriorTickets(tickets) => formatter
                .debug_struct("TrustedPriorTickets")
                .field("ticket_count", &tickets.len())
                .finish(),
            Self::ProofOfWork {
                target_cost,
                budget,
            } => formatter
                .debug_struct("ProofOfWork")
                .field("target_cost", target_cost)
                .field("budget", budget)
                .finish(),
            Self::TrustedPriorTicketsOrProofOfWork {
                tickets,
                target_cost,
                budget,
            } => formatter
                .debug_struct("TrustedPriorTicketsOrProofOfWork")
                .field("ticket_count", &tickets.len())
                .field("target_cost", target_cost)
                .field("budget", budget)
                .finish(),
        }
    }
}

/// Protocol stamp kind inferred only from its validated wire width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampKind {
    /// 16-byte ticket-derived stamp.
    Ticket,
    /// 32-byte proof-of-work stamp.
    ProofOfWork,
}

/// Stamp evidence attached to a fully validated message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampAdmission {
    /// Receiver policy explicitly did not require validation.
    NotRequired {
        /// Whether an otherwise well-formed stamp was present but not consulted.
        stamp_present: bool,
    },
    /// A stamp matched a receiver-owned prior ticket.
    TrustedPriorTicket,
    /// A proof-of-work stamp met the inclusive Python target.
    ProofOfWork {
        /// Required cost.
        target_cost: RequiredStampCost,
        /// Observed leading-zero value.
        value: u16,
    },
}

/// Source-bound, signature-verified and explicitly stamp-admitted LXMF message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ValidatedMessage<'a> {
    verified: SignatureVerifiedMessage<'a>,
    admission: StampAdmission,
}

impl<'a> ValidatedMessage<'a> {
    /// Authenticated message view.
    pub const fn message(self) -> MessageView<'a> {
        self.verified.message
    }

    /// Stamp-policy evidence used for admission.
    pub const fn stamp_admission(self) -> StampAdmission {
        self.admission
    }
}

impl fmt::Debug for ValidatedMessage<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedMessage")
            .field("message", &self.verified.message)
            .field("stamp_admission", &self.admission)
            .finish()
    }
}

/// Failure applying receiver-owned destination and stamp policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampPolicyError {
    /// Generic complete parsing did not bind the explicit destination to a local carrier.
    DestinationUnbound,
    /// Policy requires a stamp, but no fifth payload item was present.
    MissingStamp,
    /// Stamp width selected a different protocol kind than the policy requires.
    WrongStampKind {
        expected: StampKind,
        actual: StampKind,
    },
    /// Cryptographic stamp calculation completed and did not meet policy.
    Rejected { kind: StampKind },
    /// Stamp validation could not run under the supplied parameters or budget.
    Validation(StampError),
}

/// Receiver-required Python LXMF stamp cost, restricted to policy-valid values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredStampCost(u8);

impl RequiredStampCost {
    /// Construct a required cost in Python LXMF's enabled range `1..=254`.
    pub const fn new(cost: u16) -> Result<Self, StampCostError> {
        if cost > 0 && cost < 255 {
            Ok(Self(cost as u8))
        } else {
            Err(StampCostError { actual: cost })
        }
    }

    /// Numeric target cost.
    pub const fn get(self) -> u16 {
        self.0 as u16
    }
}

/// Invalid receiver-required stamp cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StampCostError {
    /// Rejected numeric value.
    pub actual: u16,
}

fn stamp_kind(stamp: Option<&[u8]>) -> Result<(StampKind, &[u8]), StampPolicyError> {
    let stamp = stamp.ok_or(StampPolicyError::MissingStamp)?;
    let kind = if stamp.len() == TICKET_LENGTH {
        StampKind::Ticket
    } else {
        StampKind::ProofOfWork
    };
    Ok((kind, stamp))
}

fn require_stamp_kind(
    stamp: Option<&[u8]>,
    expected: StampKind,
) -> Result<&[u8], StampPolicyError> {
    let (actual, stamp) = stamp_kind(stamp)?;
    if actual != expected {
        return Err(StampPolicyError::WrongStampKind { expected, actual });
    }
    Ok(stamp)
}

fn any_ticket_valid(
    stamp: &[u8],
    message_id: &[u8; MESSAGE_ID_LENGTH],
    tickets: &[[u8; TICKET_LENGTH]],
) -> bool {
    let mut accepted = 0_u8;
    for ticket in tickets {
        accepted |= u8::from(validate_ticket_stamp(stamp, message_id, ticket));
    }
    accepted != 0
}

/// A 64-byte RNS public key proven to derive one `lxmf.delivery` source hash.
pub struct BoundSourceIdentity<'a> {
    public_key: &'a [u8; IDENTITY_PUBLIC_KEY_LENGTH],
    destination_hash: [u8; DESTINATION_HASH_LENGTH],
    verifying_key: VerifyingKey,
}

impl<'a> BoundSourceIdentity<'a> {
    fn new(
        public_key: &'a [u8; IDENTITY_PUBLIC_KEY_LENGTH],
        source_hash: &[u8; DESTINATION_HASH_LENGTH],
    ) -> Result<Self, SignatureError> {
        let identity_hash = Sha256::digest(public_key);
        let mut destination_hasher = Sha256::new();
        destination_hasher.update(LXMF_DELIVERY_NAME_HASH);
        destination_hasher.update(&identity_hash[..DESTINATION_HASH_LENGTH]);
        let derived = destination_hasher.finalize();
        if derived[..DESTINATION_HASH_LENGTH]
            .ct_eq(source_hash)
            .unwrap_u8()
            != 1
        {
            return Err(SignatureError::SourceHashMismatch);
        }

        let signing_bytes: &[u8; 32] = public_key[32..]
            .try_into()
            .map_err(|_| SignatureError::InvalidPublicKey)?;
        match require_non_identity_prime_order_point(signing_bytes) {
            Ok(()) => {}
            Err(EdwardsPointError::InvalidEncoding) => {
                return Err(SignatureError::InvalidPublicKey);
            }
            Err(EdwardsPointError::NonPrimeOrder) => {
                return Err(SignatureError::NonPrimeOrderPublicKey);
            }
        }
        let verifying_key = VerifyingKey::from_bytes(signing_bytes)
            .map_err(|_| SignatureError::InvalidPublicKey)?;
        let mut destination_hash = [0_u8; DESTINATION_HASH_LENGTH];
        destination_hash.copy_from_slice(&derived[..DESTINATION_HASH_LENGTH]);
        Ok(Self {
            public_key,
            destination_hash,
            verifying_key,
        })
    }

    /// Bound 64-byte RNS public key.
    pub const fn public_key(&self) -> &'a [u8; IDENTITY_PUBLIC_KEY_LENGTH] {
        self.public_key
    }

    /// Derived `lxmf.delivery` destination hash.
    pub const fn destination_hash(&self) -> &[u8; DESTINATION_HASH_LENGTH] {
        &self.destination_hash
    }
}

impl fmt::Debug for BoundSourceIdentity<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundSourceIdentity")
            .field("public_key_len", &self.public_key.len())
            .field("destination_hash_len", &self.destination_hash.len())
            .finish()
    }
}

/// Failure constructing or verifying an LXMF signature input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// The public key does not derive the message's source destination hash.
    SourceHashMismatch,
    /// The Ed25519 half of the RNS public key is invalid.
    InvalidPublicKey,
    /// The Ed25519 public key is not a non-identity point in the prime-order subgroup.
    NonPrimeOrderPublicKey,
    /// Signature R does not encode an Edwards point.
    InvalidSignaturePoint,
    /// Signature R is not a non-identity point in the prime-order subgroup.
    NonPrimeOrderSignaturePoint,
    /// Ed25519 verification failed.
    InvalidSignature,
}

/// Caller-owned output storage was too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferError {
    /// Required byte count.
    pub required: usize,
    /// Available byte count.
    pub available: usize,
}

/// Requested application delivery lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredDelivery {
    /// Destination DATA without a link when it fits.
    Opportunistic,
    /// Link-based delivery.
    Direct,
}

/// Effective LXMF delivery method after threshold fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMethod {
    /// Destination DATA without a link.
    Opportunistic,
    /// Link-based delivery.
    Direct,
}

/// LXMF representation selected inside the effective method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representation {
    /// One RNS packet.
    Packet,
    /// One RNS Resource.
    Resource,
}

/// Transport-neutral delivery decision matching pinned Python LXMF 1.1.0 thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryDecision {
    /// Effective method.
    pub method: DeliveryMethod,
    /// Packet or Resource representation.
    pub representation: Representation,
}

/// Select Python LXMF's encrypted opportunistic/direct delivery lane.
pub const fn select_delivery(
    desired: DesiredDelivery,
    selection_content_size: usize,
) -> DeliveryDecision {
    match desired {
        DesiredDelivery::Opportunistic
            if selection_content_size <= ENCRYPTED_PACKET_MAX_CONTENT =>
        {
            DeliveryDecision {
                method: DeliveryMethod::Opportunistic,
                representation: Representation::Packet,
            }
        }
        DesiredDelivery::Opportunistic | DesiredDelivery::Direct
            if selection_content_size <= LINK_PACKET_MAX_CONTENT =>
        {
            DeliveryDecision {
                method: DeliveryMethod::Direct,
                representation: Representation::Packet,
            }
        }
        DesiredDelivery::Opportunistic | DesiredDelivery::Direct => DeliveryDecision {
            method: DeliveryMethod::Direct,
            representation: Representation::Resource,
        },
    }
}

fn parse_payload(raw: &[u8], limits: WireLimits) -> Result<PayloadView<'_>, WireError> {
    let marker = *raw.first().ok_or(WireError::TooShort {
        minimum: 1,
        actual: 0,
    })?;
    let (count, header_len) = match marker {
        PAYLOAD_ARRAY_4 => (4, 1),
        PAYLOAD_ARRAY_5 => (5, 1),
        0xdc => {
            let length = raw.get(1..3).ok_or(WireError::Truncated {
                offset: 0,
                needed: 3,
                available: raw.len(),
            })?;
            (u16::from_be_bytes([length[0], length[1]]) as usize, 3)
        }
        0xdd => {
            let length = raw.get(1..5).ok_or(WireError::Truncated {
                offset: 0,
                needed: 5,
                available: raw.len(),
            })?;
            let count = u32::from_be_bytes([length[0], length[1], length[2], length[3]]);
            let count =
                usize::try_from(count).map_err(|_| WireError::LengthOverflow { offset: 0 })?;
            (count, 5)
        }
        _ => {
            return Err(WireError::InvalidPayloadArray {
                marker,
                count: None,
            });
        }
    };
    if !matches!(count, 4 | 5) {
        return Err(WireError::InvalidPayloadArray {
            marker,
            count: u32::try_from(count).ok(),
        });
    }

    let mut scanner = msgpack::Scanner::new(raw, limits);
    scanner.note_container(0, 1, count)?;
    let mut ranges = [(0_usize, 0_usize); 5];
    let mut canonicality = [msgpack::Canonicality::NonCanonical; 5];
    let mut cursor = header_len;
    for index in 0..count {
        let scanned = scanner.scan_value(cursor, 2)?;
        ranges[index] = (cursor, scanned.end);
        canonicality[index] = scanned.canonicality;
        cursor = scanned.end;
    }
    if cursor != raw.len() {
        return Err(WireError::TrailingBytes {
            offset: cursor,
            trailing: raw.len() - cursor,
        });
    }

    let item = |index: usize| &raw[ranges[index].0..ranges[index].1];
    let timestamp = item(0);
    if timestamp.first().copied() != Some(MSGPACK_F64) || timestamp.len() != 9 {
        return Err(WireError::InvalidTimestamp {
            marker: timestamp.first().copied().unwrap_or(0),
        });
    }
    let timestamp_bits =
        u64::from_be_bytes(
            timestamp[1..]
                .try_into()
                .map_err(|_| WireError::InvalidTimestamp {
                    marker: timestamp.first().copied().unwrap_or(0),
                })?,
        );
    let title = parse_binary(item(1), true)?;
    let content = parse_binary(item(2), false)?;
    let fields_raw = item(3);
    let fields_marker = fields_raw[0];
    if !matches!(fields_marker, 0x80..=0x8f | 0xde | 0xdf) {
        return Err(WireError::InvalidFields {
            marker: fields_marker,
        });
    }
    let fields = MessagePackValue::from_scanned(fields_raw, MessagePackKind::Map, canonicality[3]);

    let stamp = if count == 5 {
        if !canonicality[..4]
            .iter()
            .all(|value| *value == msgpack::Canonicality::Canonical)
        {
            return Err(WireError::NonCanonicalStampedPayload);
        }
        let parsed = parse_stamp(item(4))?;
        Some(parsed)
    } else {
        None
    };

    Ok(PayloadView {
        raw,
        first_four: [item(0), item(1), item(2), item(3)],
        timestamp_bits,
        title,
        content,
        fields,
        stamp,
    })
}

fn parse_binary(raw: &[u8], title: bool) -> Result<BinaryView<'_>, WireError> {
    let marker = raw[0];
    let prefix = match marker {
        0xc4 => 2,
        0xc5 => 3,
        0xc6 => 5,
        _ if title => return Err(WireError::InvalidTitle { marker }),
        _ => return Err(WireError::InvalidContent { marker }),
    };
    Ok(BinaryView {
        bytes: &raw[prefix..],
        encoded_len: raw.len(),
    })
}

fn parse_stamp(raw: &[u8]) -> Result<BinaryView<'_>, WireError> {
    let marker = raw[0];
    let prefix = match marker {
        0xc4 => 2,
        0xc5 => 3,
        0xc6 => 5,
        _ => return Err(WireError::InvalidStamp { marker, length: 0 }),
    };
    let bytes = &raw[prefix..];
    if !matches!(bytes.len(), TICKET_LENGTH | POW_STAMP_LENGTH) {
        return Err(WireError::InvalidStamp {
            marker,
            length: bytes.len(),
        });
    }
    Ok(BinaryView {
        bytes,
        encoded_len: raw.len(),
    })
}

fn enforce_wire_limit(actual: usize, limits: WireLimits) -> Result<(), WireError> {
    if actual > limits.max_wire_bytes {
        Err(WireError::LimitExceeded {
            kind: LimitKind::WireBytes,
            actual,
            maximum: limits.max_wire_bytes,
            offset: 0,
        })
    } else {
        Ok(())
    }
}

fn copy_part(output: &mut [u8], cursor: &mut usize, part: &[u8]) {
    let end = *cursor + part.len();
    output[*cursor..end].copy_from_slice(part);
    *cursor = end;
}
