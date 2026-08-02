//! RNS 1.4.0 `Link.request` (context 0x09) and its answer (0x0A).
//! - A request is msgpack `[time, truncated_hash(path), data]`.
//! - The response is msgpack `[request_id, data]`. `data` crosses as raw msgpack value bytes.
//!
//! Payloads past the link MDU are Resource territory, refused here.

use crate::crypto::sha256;
use crate::engine::{
    CommandId, CommandOutcome, RequestResponseTimeout, Respond, RespondData, RespondPayload,
    RespondRejection, SendRequest, SendRequestRejection, MAX_RESPOND_DATA_LEN,
    MAX_SEND_REQUEST_DATA_LEN,
};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::IdentitySigningPublicKey;
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{CulledReceipt, OutstandingReceipt, ReceiptKind};
use crate::routing::links::data::{link_mdu, link_traffic_timeout_ms};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext,
    WirePacketHeader, TRUNCATED_HASH_BYTE_LEN,
};
#[cfg(test)]
use crate::wire::{DestinationHash, BROADCAST_MTU};

/// RNS 1.4.0 `Resource.RESPONSE_MAX_GRACE_TIME` (10 s) × 1.125, the flat term in a request's default timeout: `rtt × traffic_timeout_factor + 11.25 s`.
pub const REQUEST_RESPONSE_GRACE_MS: u64 = 11_250;

/// msgpack `fixarray(3)` ‖ `float64` time ‖ `bin8(16)` path hash, before data.
pub const REQUEST_WIRE_OVERHEAD: usize = 1 + 9 + 2 + TRUNCATED_HASH_BYTE_LEN;

/// msgpack `fixarray(2)` ‖ `bin8(16)` request id, before data.
pub const RESPONSE_WIRE_OVERHEAD: usize = 1 + 2 + TRUNCATED_HASH_BYTE_LEN;

/// Both verbs' data caps derive from the single-packet link MDU, so the two sides land on the same figure by construction.
pub const WRAPPED_PLAINTEXT_CAP: usize = {
    let request = REQUEST_WIRE_OVERHEAD + MAX_SEND_REQUEST_DATA_LEN;
    let response = RESPONSE_WIRE_OVERHEAD + MAX_RESPOND_DATA_LEN;
    if request > response {
        request
    } else {
        response
    }
};

const FIXARRAY_3: u8 = 0x93;
const FIXARRAY_2: u8 = 0x92;
const FLOAT_64: u8 = 0xCB;
const BIN_8: u8 = 0xC4;
const BIN_16: u8 = 0xC5;
const BIN_32: u8 = 0xC6;
const NIL: u8 = 0xC0;
pub const MAX_PACKED_BINARY_HEADER_LEN: usize = 5;

/// RNS 1.4.0 `packet.getTruncatedHash()`, naming the request in its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub [u8; TRUNCATED_HASH_BYTE_LEN]);

impl RequestId {
    #[must_use]
    pub fn of_packet(packet_hash: &PacketHash) -> Self {
        let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
        id.copy_from_slice(&packet_hash.as_bytes()[..TRUNCATED_HASH_BYTE_LEN]);
        Self(id)
    }

    /// RNS 1.4.0 `Link.request` / `request_resource_concluded`: a request that rode a resource is named by `truncated_hash(packed_request)`
    #[must_use]
    pub fn of_request_data(packed_request: &[u8]) -> Self {
        let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
        id.copy_from_slice(&sha256(packed_request)[..TRUNCATED_HASH_BYTE_LEN]);
        Self(id)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPlaintextError {
    BufferTooShort,
    Malformed,
}

pub fn write_request_plaintext(
    now: InstantMillis,
    path_hash: &RequestPathHash,
    data: &[u8],
    buf: &mut [u8],
) -> Result<usize, RequestPlaintextError> {
    let data_len = if data.is_empty() { 1 } else { data.len() };
    let total = REQUEST_WIRE_OVERHEAD + data_len;
    if buf.len() < total {
        return Err(RequestPlaintextError::BufferTooShort);
    }
    buf[0] = FIXARRAY_3;
    buf[1] = FLOAT_64;
    let seconds = now.0 as f64 / 1_000.0;
    buf[2..10].copy_from_slice(&seconds.to_be_bytes());
    buf[10] = BIN_8;
    buf[11] = TRUNCATED_HASH_BYTE_LEN as u8;
    buf[12..12 + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(path_hash.as_bytes());
    if data.is_empty() {
        buf[REQUEST_WIRE_OVERHEAD] = NIL;
    } else {
        buf[REQUEST_WIRE_OVERHEAD..total].copy_from_slice(data);
    }
    Ok(total)
}

#[derive(Debug)]
pub struct ParsedRequest<'a> {
    pub requested_at: InstantMillis,
    pub path_hash: RequestPathHash,
    pub data: &'a [u8],
}

/// Hostile floats saturate the way the LRRTT parse does; anything not shaped like the reference's three-element pack is refused.
pub fn parse_request_plaintext(
    plaintext: &[u8],
) -> Result<ParsedRequest<'_>, RequestPlaintextError> {
    if plaintext.len() < REQUEST_WIRE_OVERHEAD + 1 {
        return Err(RequestPlaintextError::Malformed);
    }
    if plaintext[0] != FIXARRAY_3 || plaintext[1] != FLOAT_64 {
        return Err(RequestPlaintextError::Malformed);
    }
    let mut seconds = [0u8; 8];
    seconds.copy_from_slice(&plaintext[2..10]);
    let seconds = f64::from_be_bytes(seconds);
    let requested_at = (seconds * 1_000.0 + 0.5) as u64;
    if plaintext[10] != BIN_8 || plaintext[11] != TRUNCATED_HASH_BYTE_LEN as u8 {
        return Err(RequestPlaintextError::Malformed);
    }
    let mut path_hash = [0u8; TRUNCATED_HASH_BYTE_LEN];
    path_hash.copy_from_slice(&plaintext[12..12 + TRUNCATED_HASH_BYTE_LEN]);
    Ok(ParsedRequest {
        requested_at: InstantMillis(requested_at),
        path_hash: RequestPathHash::new(path_hash),
        data: &plaintext[REQUEST_WIRE_OVERHEAD..],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsePlaintextError {
    BufferTooShort,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackBinaryError {
    BufferTooShort,
    LengthOutOfRange,
}

pub const fn packed_binary_header_len(byte_len: usize) -> Option<usize> {
    match byte_len {
        0..=0xFF => Some(2),
        0x100..=0xFFFF => Some(3),
        0x1_0000..=0xFFFF_FFFF => Some(5),
        _ => None,
    }
}

pub const fn packed_binary_len(byte_len: usize) -> Option<usize> {
    match packed_binary_header_len(byte_len) {
        Some(header_len) => header_len.checked_add(byte_len),
        None => None,
    }
}

pub fn write_packed_binary_header(
    byte_len: usize,
    output: &mut [u8],
) -> Result<usize, PackBinaryError> {
    let header_len = packed_binary_header_len(byte_len).ok_or(PackBinaryError::LengthOutOfRange)?;
    if output.len() < header_len {
        return Err(PackBinaryError::BufferTooShort);
    }
    match header_len {
        2 => {
            output[0] = BIN_8;
            output[1] = byte_len as u8;
        }
        3 => {
            output[0] = BIN_16;
            output[1..3].copy_from_slice(&(byte_len as u16).to_be_bytes());
        }
        5 => {
            output[0] = BIN_32;
            output[1..5].copy_from_slice(&(byte_len as u32).to_be_bytes());
        }
        _ => unreachable!(),
    }
    Ok(header_len)
}

/// `umsgpack.packb([request_id, response])`.
/// An empty response body still rides as one msgpack `nil` byte.
pub const fn response_data_wire_len(data_len: usize) -> usize {
    if data_len == 0 {
        1
    } else {
        data_len
    }
}

pub fn write_response_plaintext(
    request_id: &RequestId,
    data: &[u8],
    buf: &mut [u8],
) -> Result<usize, ResponsePlaintextError> {
    let data_len = response_data_wire_len(data.len());
    let total = RESPONSE_WIRE_OVERHEAD + data_len;
    if buf.len() < total {
        return Err(ResponsePlaintextError::BufferTooShort);
    }
    buf[0] = FIXARRAY_2;
    buf[1] = BIN_8;
    buf[2] = TRUNCATED_HASH_BYTE_LEN as u8;
    buf[3..3 + TRUNCATED_HASH_BYTE_LEN].copy_from_slice(request_id.as_bytes());
    if data.is_empty() {
        buf[RESPONSE_WIRE_OVERHEAD] = NIL;
    } else {
        buf[RESPONSE_WIRE_OVERHEAD..total].copy_from_slice(data);
    }
    Ok(total)
}

pub fn response_envelope_prefix(request_id: &RequestId) -> [u8; RESPONSE_WIRE_OVERHEAD] {
    let mut prefix = [0u8; RESPONSE_WIRE_OVERHEAD];
    prefix[0] = FIXARRAY_2;
    prefix[1] = BIN_8;
    prefix[2] = TRUNCATED_HASH_BYTE_LEN as u8;
    prefix[3..].copy_from_slice(request_id.as_bytes());
    prefix
}

pub fn parse_response_plaintext(
    plaintext: &[u8],
) -> Result<(RequestId, &[u8]), ResponsePlaintextError> {
    if plaintext.len() < RESPONSE_WIRE_OVERHEAD + 1 {
        return Err(ResponsePlaintextError::Malformed);
    }
    if plaintext[0] != FIXARRAY_2
        || plaintext[1] != BIN_8
        || plaintext[2] != TRUNCATED_HASH_BYTE_LEN as u8
    {
        return Err(ResponsePlaintextError::Malformed);
    }
    let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
    id.copy_from_slice(&plaintext[3..3 + TRUNCATED_HASH_BYTE_LEN]);
    Ok((RequestId(id), &plaintext[RESPONSE_WIRE_OVERHEAD..]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendRequestDispatch {
    pub wire_bytes: usize,
    pub fire_on: InterfaceId,
    pub request_id: RequestId,
    pub culled: Option<CulledReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespondDispatch {
    pub wire_bytes: usize,
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRequestWriteError {
    LinkVanished,
    PayloadTooLong,
    BufferTooShort,
}

pub(crate) fn request_response_timeout_ms(rtt: RttMillis) -> u64 {
    link_traffic_timeout_ms(rtt).saturating_add(REQUEST_RESPONSE_GRACE_MS)
}

fn requested_response_timeout_ms(rtt: RttMillis, timeout: RequestResponseTimeout) -> u64 {
    match timeout {
        RequestResponseTimeout::LinkDefault => request_response_timeout_ms(rtt),
        RequestResponseTimeout::Exact(timeout) => timeout.0,
    }
}

fn seal_link_frame(
    link_id: &LinkId,
    key: &LinkKey,
    context: WireContext,
    plaintext: &[u8],
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Option<(usize, usize)> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link_id.to_address(),
        context,
    };
    let header_len = header.write(buf).ok()?;
    let sealed = key.seal(iv, plaintext, &mut buf[header_len..]).ok()?;
    Some((header_len, header_len + sealed))
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_request(&self, id: CommandId, request: SendRequest) -> CommandOutcome {
        match self.links.phase_for(&request.link_id) {
            None => CommandOutcome::SendRequestRejected {
                id,
                rejection: SendRequestRejection::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::SendRequestRejected {
                    id,
                    rejection: SendRequestRejection::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => CommandOutcome::OwesSendRequest { id, request },
        }
    }

    pub fn ingest_respond(&self, id: CommandId, respond: Respond) -> CommandOutcome {
        match self.links.phase_for(&respond.link_id) {
            None => CommandOutcome::RespondRejected {
                id,
                rejection: RespondRejection::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::RespondRejected {
                    id,
                    rejection: RespondRejection::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => match &respond.payload {
                RespondPayload::Packed(_) => CommandOutcome::OwesRespond { id, respond },
                RespondPayload::StaticBytes(bytes) => {
                    let bytes = *bytes;
                    let Some(packed_len) = packed_binary_len(bytes.len()) else {
                        return CommandOutcome::OwesResourceResponse { id, respond };
                    };
                    if !self.response_data_len_fits_packet(&respond.link_id, packed_len) {
                        return CommandOutcome::OwesResourceResponse { id, respond };
                    }
                    let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
                    let Ok(header_len) = write_packed_binary_header(bytes.len(), &mut header)
                    else {
                        return CommandOutcome::OwesResourceResponse { id, respond };
                    };
                    let mut data = RespondData::new();
                    if data.extend_from_slice(&header[..header_len]).is_err()
                        || data.extend_from_slice(bytes).is_err()
                    {
                        return CommandOutcome::OwesResourceResponse { id, respond };
                    }
                    CommandOutcome::OwesRespond {
                        id,
                        respond: Respond {
                            link_id: respond.link_id,
                            request_id: respond.request_id,
                            payload: RespondPayload::Packed(data),
                        },
                    }
                }
                #[cfg(any(feature = "large-static-responses", test))]
                RespondPayload::StaticFile { .. } => {
                    CommandOutcome::OwesResourceResponse { id, respond }
                }
            },
        }
    }

    pub fn response_fits_packet(&self, link_id: &LinkId, data: &[u8]) -> bool {
        self.response_data_len_fits_packet(link_id, data.len())
    }

    pub fn response_data_len_fits_packet(&self, link_id: &LinkId, data_len: usize) -> bool {
        let Some(LinkPhase::Active { mtu, .. }) = self.links.phase_for(link_id) else {
            return false;
        };
        let wire_data_len = response_data_wire_len(data_len);
        RESPONSE_WIRE_OVERHEAD + wire_data_len <= link_mdu(*mtu) && data_len <= MAX_RESPOND_DATA_LEN
    }

    pub fn request_fits_packet(&self, link_id: &LinkId, data: &[u8]) -> bool {
        let Some(LinkPhase::Active { mtu, .. }) = self.links.phase_for(link_id) else {
            return false;
        };
        let data_len = if data.is_empty() { 1 } else { data.len() };
        REQUEST_WIRE_OVERHEAD + data_len <= link_mdu(*mtu)
            && data.len() <= MAX_SEND_REQUEST_DATA_LEN
    }

    pub fn write_commanded_send_request(
        &mut self,
        id: CommandId,
        request: &SendRequest,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<SendRequestDispatch, LinkRequestWriteError> {
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt,
            peer_signing,
            ..
        }) = self.links.phase_for(&request.link_id)
        else {
            return Err(LinkRequestWriteError::LinkVanished);
        };
        let fire_on = *attached_interface;
        let peer_signing = *peer_signing;
        let timeout_ms = requested_response_timeout_ms(*rtt, request.response_timeout);

        let mut plaintext = [0u8; WRAPPED_PLAINTEXT_CAP];
        let plain_len =
            write_request_plaintext(now, &request.path_hash, &request.data, &mut plaintext)
                .map_err(|_| LinkRequestWriteError::BufferTooShort)?;
        if plain_len > link_mdu(*mtu) {
            return Err(LinkRequestWriteError::PayloadTooLong);
        }
        let (header_len, wire_bytes) = seal_link_frame(
            &request.link_id,
            key,
            WireContext::Request,
            &plaintext[..plain_len],
            iv,
            buf,
        )
        .ok_or(LinkRequestWriteError::BufferTooShort)?;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Link,
            &request.link_id.to_address(),
            WireContext::Request,
            &buf[header_len..wire_bytes],
        );
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: id,
            kind: ReceiptKind::SendRequest {
                maximum_response_bytes: request.maximum_response_bytes,
            },
            peer_signing_key: IdentitySigningPublicKey::new(peer_signing),
            sent_at: now,
            timeout_at: InstantMillis(now.0.saturating_add(timeout_ms)),
        });

        Ok(SendRequestDispatch {
            wire_bytes,
            fire_on,
            request_id: RequestId::of_packet(&packet_hash),
            culled,
        })
    }

    /// The request formed no packet, so the row is keyed by `sha256` of the pack. Its first sixteen bytes are the request id (RNS 1.4.0 `truncated_hash(packed_request)`) the response names back.
    pub(crate) fn book_request_resource_receipt(
        &mut self,
        id: CommandId,
        link_id: &LinkId,
        packed_request: &[u8],
        response_timeout: RequestResponseTimeout,
        maximum_response_bytes: crate::units::ByteLimit,
        now: InstantMillis,
    ) {
        let Some(LinkPhase::Active {
            rtt, peer_signing, ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let peer_signing = *peer_signing;
        let timeout_ms = requested_response_timeout_ms(*rtt, response_timeout);
        let _ = self.receipts.track(OutstandingReceipt {
            packet_hash: PacketHash::new(sha256(packed_request)),
            command_id: id,
            kind: ReceiptKind::SendRequest {
                maximum_response_bytes,
            },
            peer_signing_key: IdentitySigningPublicKey::new(peer_signing),
            sent_at: now,
            timeout_at: InstantMillis(now.0.saturating_add(timeout_ms)),
        });
    }

    /// Fire-and-forget; the reference sends its response packet and moves on.
    pub fn write_commanded_respond(
        &self,
        respond: &Respond,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<RespondDispatch, LinkRequestWriteError> {
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            ..
        }) = self.links.phase_for(&respond.link_id)
        else {
            return Err(LinkRequestWriteError::LinkVanished);
        };
        let RespondPayload::Packed(data) = &respond.payload else {
            return Err(LinkRequestWriteError::PayloadTooLong);
        };
        let mut plaintext = [0u8; WRAPPED_PLAINTEXT_CAP];
        let plain_len = write_response_plaintext(&respond.request_id, data, &mut plaintext)
            .map_err(|_| LinkRequestWriteError::BufferTooShort)?;
        if plain_len > link_mdu(*mtu) {
            return Err(LinkRequestWriteError::PayloadTooLong);
        }
        let (_, wire_bytes) = seal_link_frame(
            &respond.link_id,
            key,
            WireContext::Response,
            &plaintext[..plain_len],
            iv,
            buf,
        )
        .ok_or(LinkRequestWriteError::BufferTooShort)?;
        Ok(RespondDispatch {
            wire_bytes,
            fire_on: *attached_interface,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::table::LinkActivation;

    const PATH_HASH: RequestPathHash = RequestPathHash::new([0x5A; 16]);

    #[test]
    fn the_response_envelope_prefix_matches_the_packed_response_head() {
        assert_eq!(
            WRAPPED_PLAINTEXT_CAP,
            REQUEST_WIRE_OVERHEAD + MAX_SEND_REQUEST_DATA_LEN,
        );
        assert_eq!(
            WRAPPED_PLAINTEXT_CAP,
            RESPONSE_WIRE_OVERHEAD + MAX_RESPOND_DATA_LEN,
        );
        let id = RequestId([0x5A; 16]);
        let mut buf = [0u8; 64];
        let total = write_response_plaintext(&id, &[0xA3, b'a', b'b', b'c'], &mut buf).unwrap();
        assert_eq!(response_envelope_prefix(&id), buf[..RESPONSE_WIRE_OVERHEAD]);
        assert_eq!(total, RESPONSE_WIRE_OVERHEAD + 4);
    }

    #[test]
    fn the_request_pack_is_byte_identical_to_umsgpack() {
        // umsgpack.packb([2.5, bytes([0x5A]*16), b"\xA3abc"]): fixarray(3), float64 2.5, bin8(16), then the data bytes verbatim (here a fixstr).
        let mut buf = [0u8; 64];
        let n = write_request_plaintext(
            InstantMillis(2_500),
            &PATH_HASH,
            &[0xA3, b'a', b'b', b'c'],
            &mut buf,
        )
        .unwrap();
        let mut expected = std::vec![0x93, 0xCB];
        expected.extend_from_slice(&2.5f64.to_be_bytes());
        expected.extend_from_slice(&[0xC4, 0x10]);
        expected.extend_from_slice(PATH_HASH.as_bytes());
        expected.extend_from_slice(&[0xA3, b'a', b'b', b'c']);
        assert_eq!(&buf[..n], expected.as_slice());

        let parsed = parse_request_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.requested_at, InstantMillis(2_500));
        assert_eq!(parsed.path_hash, PATH_HASH);
        assert_eq!(parsed.data, &[0xA3, b'a', b'b', b'c']);
    }

    #[test]
    fn an_empty_request_packs_the_reference_none_as_nil() {
        let mut buf = [0u8; 64];
        let n = write_request_plaintext(InstantMillis(1_000), &PATH_HASH, &[], &mut buf).unwrap();
        assert_eq!(buf[n - 1], 0xC0);
        let parsed = parse_request_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.data, &[0xC0]);
    }

    #[test]
    fn the_response_pack_round_trips_and_names_the_id() {
        let id = RequestId([0x7E; 16]);
        let mut buf = [0u8; 64];
        let n = write_response_plaintext(&id, &[0xC4, 0x02, 0xAA, 0xBB], &mut buf).unwrap();
        assert_eq!(&buf[..3], &[0x92, 0xC4, 0x10]);
        let (parsed_id, data) = parse_response_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed_id, id);
        assert_eq!(data, &[0xC4, 0x02, 0xAA, 0xBB]);
    }

    #[test]
    fn hostile_floats_saturate_and_malformed_packs_refuse() {
        let mut buf = [0u8; 64];
        let n =
            write_request_plaintext(InstantMillis(1_000), &PATH_HASH, &[0xC0], &mut buf).unwrap();
        buf[2..10].copy_from_slice(&f64::NAN.to_be_bytes());
        assert_eq!(
            parse_request_plaintext(&buf[..n]).unwrap().requested_at,
            InstantMillis(0),
        );
        buf[0] = 0x92;
        assert_eq!(
            parse_request_plaintext(&buf[..n]).unwrap_err(),
            RequestPlaintextError::Malformed,
        );
        assert_eq!(
            parse_response_plaintext(&[0x92, 0xC4]).unwrap_err(),
            ResponsePlaintextError::Malformed,
        );
    }

    #[test]
    fn write_request_plaintext_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let exact = REQUEST_WIRE_OVERHEAD + 1;
        let mut fits = std::vec![0u8; exact];
        assert_eq!(
            write_request_plaintext(InstantMillis(1_000), &PATH_HASH, &[], &mut fits),
            Ok(exact),
        );
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(
            write_request_plaintext(InstantMillis(1_000), &PATH_HASH, &[], &mut short),
            Err(RequestPlaintextError::BufferTooShort),
        );
    }

    #[test]
    fn write_response_plaintext_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let id = RequestId([0x7E; 16]);
        let exact = RESPONSE_WIRE_OVERHEAD + 1;
        let mut fits = std::vec![0u8; exact];
        assert_eq!(write_response_plaintext(&id, &[], &mut fits), Ok(exact));
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(
            write_response_plaintext(&id, &[], &mut short),
            Err(ResponsePlaintextError::BufferTooShort),
        );
    }

    fn valid_request_plaintext() -> [u8; REQUEST_WIRE_OVERHEAD + 1] {
        let mut buf = [0u8; REQUEST_WIRE_OVERHEAD + 1];
        write_request_plaintext(InstantMillis(1_000), &PATH_HASH, &[], &mut buf).unwrap();
        buf
    }

    #[test]
    fn parse_request_plaintext_refuses_each_header_gate_independently() {
        assert!(parse_request_plaintext(&valid_request_plaintext()).is_ok());
        assert_eq!(
            parse_request_plaintext(&valid_request_plaintext()[..REQUEST_WIRE_OVERHEAD])
                .unwrap_err(),
            RequestPlaintextError::Malformed,
        );
        for (index, wrong) in [(0, 0x92u8), (1, 0xC4), (10, 0xCB), (11, 0x0F)] {
            let mut bytes = valid_request_plaintext();
            bytes[index] = wrong;
            assert_eq!(
                parse_request_plaintext(&bytes).unwrap_err(),
                RequestPlaintextError::Malformed,
                "byte {index} must be checked on its own",
            );
        }
    }

    fn engine_with_an_active_link_at(
        link_id: LinkId,
        mtu: usize,
    ) -> EngineState<crate::storage::GrowableHeap> {
        use crate::crypto::{
            x25519_diffie_hellman, Ed25519PublicKey, Ed25519SecretKey, X25519PublicKey,
            X25519SecretKey,
        };
        use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
        use crate::routing::links::table::InitiatedLink;

        let mut engine = EngineState::<crate::storage::GrowableHeap>::new(Zeroizing::new(
            [0x07; IDENTITY_SECRET_KEY_LEN],
        ));
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x11; 16]),
                expected_hops: 1,
                mode: crate::routing::links::LinkMode::Aes256Cbc,
                initiator_secret: X25519SecretKey::new([0x21; 32]),
                link_signing: Ed25519SecretKey::new([0x21; 32]),
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(600_000),
                command_id: CommandId(1),
            })
            .unwrap();
        let key = LinkKey::derive(
            &link_id,
            &x25519_diffie_hellman(
                &X25519SecretKey::new([0x21; 32]),
                &X25519PublicKey([0x63; 32]),
            ),
        );
        engine
            .links
            .activate_initiated(
                &link_id,
                key,
                &LinkActivation {
                    received_hops: 1,
                    rtt: crate::units::RttMillis::new(100),
                    mtu,
                    attached_interface: InterfaceId::new([0xEE; 8]),
                    peer_signing: Ed25519PublicKey([0x5A; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();
        engine
    }

    fn engine_with_request_limit(
        maximum_request_bytes: crate::units::ByteLimit,
    ) -> EngineState<crate::storage::GrowableHeap> {
        use crate::crypto::ratchets::RatchetPolicy;
        use crate::crypto::Ed25519PublicKey;
        use crate::identity::IdentityHash;
        use crate::routing::links::resources::receive::tests_support::{lane, link_id, link_key};
        use crate::routing::links::table::RespondingLink;
        use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};

        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let identity = IdentityHash::new([0x77; 16]);
        let destination = engine
            .upstream_app_destinations
            .register_single(
                &identity,
                "limits",
                &["request"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        assert!(engine.set_maximum_request_bytes(&destination, maximum_request_bytes));
        engine
            .request_handlers
            .register(
                destination,
                PATH_HASH,
                crate::routing::request_handlers::RequestPolicy::AllowAll,
            )
            .unwrap();
        engine
            .links
            .track_responding(RespondingLink {
                link_id: link_id(),
                key: link_key(),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                mtu: BROADCAST_MTU,
                initiator_signing: Ed25519PublicKey([0x99; 32]),
                destination,
                identity,
                proof_strategy: ProofStrategy::ProveNone,
            })
            .unwrap();
        engine
            .links
            .activate_responding(
                &link_id(),
                crate::units::RttMillis::new(250),
                lane(),
                InstantMillis(1_000),
            )
            .unwrap();
        engine
    }

    #[test]
    fn packet_requests_are_admitted_at_the_destination_limit_and_refused_past_it() {
        use crate::routing::links::data::write_link_packet;
        use crate::routing::links::resources::receive::tests_support::{feed, link_id, link_key};
        use crate::units::ByteLimit;

        let request_data = [BIN_8, 3, b'a', b's', b'k'];
        let mut plaintext = [0u8; 64];
        let plaintext_len = write_request_plaintext(
            InstantMillis(1_500),
            &PATH_HASH,
            &request_data,
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::Request,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();

        let mut exact = engine_with_request_limit(ByteLimit::Maximum(plaintext_len as u64));
        let admitted = feed(&mut exact, &frame[..wire_bytes], 2_000);
        assert_eq!(admitted.requests.len(), 1);
        assert_eq!(admitted.requests[0].1, request_data);

        let mut over = engine_with_request_limit(ByteLimit::Maximum(plaintext_len as u64 - 1));
        let refused = feed(&mut over, &frame[..wire_bytes], 2_000);
        assert!(refused.requests.is_empty());
    }

    #[test]
    fn write_commanded_send_request_admits_an_exact_mdu_payload_and_refuses_one_byte_past() {
        use crate::engine::SendRequestData;
        let link_id = LinkId::new([0x42; 16]);
        let mdu = link_mdu(300);
        let send = |data_len: usize| {
            let request = SendRequest {
                link_id,
                path_hash: RequestPathHash::of("/q"),
                data: SendRequestData::from_slice(&std::vec![0xAA; data_len]).unwrap(),
                response_timeout: Default::default(),
                maximum_response_bytes: Default::default(),
            };
            let mut buf = [0u8; 600];
            engine_with_an_active_link_at(link_id, 300).write_commanded_send_request(
                CommandId(2),
                &request,
                InstantMillis(2_000),
                &[0u8; 16],
                &mut buf,
            )
        };
        assert!(send(mdu - REQUEST_WIRE_OVERHEAD).is_ok());
        assert_eq!(
            send(mdu - REQUEST_WIRE_OVERHEAD + 1).map(|_| ()),
            Err(LinkRequestWriteError::PayloadTooLong),
        );
    }

    #[test]
    fn the_default_request_response_timeout_owns_the_receipt_deadline() {
        use crate::engine::SendRequestData;

        assert_eq!(request_response_timeout_ms(RttMillis::new(100)), 12_850);
        let link_id = LinkId::new([0x42; 16]);
        let mut engine = engine_with_an_active_link_at(link_id, 300);
        let request = SendRequest {
            link_id,
            path_hash: RequestPathHash::of("/default-timeout"),
            data: SendRequestData::from_slice(b"work").unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: Default::default(),
        };
        let mut buf = [0u8; 600];
        engine
            .write_commanded_send_request(
                CommandId(2),
                &request,
                InstantMillis(2_000),
                &[0u8; 16],
                &mut buf,
            )
            .unwrap();
        assert_eq!(
            engine.receipts.earliest_timeout_at(),
            Some(InstantMillis(14_850)),
        );
    }

    #[test]
    fn an_explicit_request_response_timeout_owns_the_receipt_deadline() {
        use crate::engine::SendRequestData;
        use crate::units::DurationMillis;

        let link_id = LinkId::new([0x42; 16]);
        let mut engine = engine_with_an_active_link_at(link_id, 300);
        let request = SendRequest {
            link_id,
            path_hash: RequestPathHash::of("/slow"),
            data: SendRequestData::from_slice(b"work").unwrap(),
            response_timeout: RequestResponseTimeout::Exact(DurationMillis(45_000)),
            maximum_response_bytes: Default::default(),
        };
        let mut buf = [0u8; 600];
        engine
            .write_commanded_send_request(
                CommandId(2),
                &request,
                InstantMillis(2_000),
                &[0u8; 16],
                &mut buf,
            )
            .unwrap();
        assert_eq!(
            engine.receipts.earliest_timeout_at(),
            Some(InstantMillis(47_000))
        );
    }

    #[test]
    fn write_commanded_respond_admits_an_exact_mdu_payload_and_refuses_one_byte_past() {
        use crate::engine::RespondData;
        let link_id = LinkId::new([0x43; 16]);
        let mdu = link_mdu(300);
        let respond = |data_len: usize| {
            let respond = Respond {
                link_id,
                request_id: RequestId([0x7E; 16]),
                payload: RespondPayload::Packed(
                    RespondData::from_slice(&std::vec![0xBB; data_len]).unwrap(),
                ),
            };
            let mut buf = [0u8; 600];
            engine_with_an_active_link_at(link_id, 300)
                .write_commanded_respond(&respond, &[0u8; 16], &mut buf)
        };
        assert!(respond(mdu - RESPONSE_WIRE_OVERHEAD).is_ok());
        assert_eq!(
            respond(mdu - RESPONSE_WIRE_OVERHEAD + 1).map(|_| ()),
            Err(LinkRequestWriteError::PayloadTooLong),
        );
    }

    #[test]
    fn binary_headers_select_the_smallest_message_pack_width() {
        for (byte_len, expected) in [
            (0, &[0xC4, 0][..]),
            (u8::MAX as usize, &[0xC4, 0xFF][..]),
            (u8::MAX as usize + 1, &[0xC5, 0x01, 0x00][..]),
            (u16::MAX as usize, &[0xC5, 0xFF, 0xFF][..]),
            (u16::MAX as usize + 1, &[0xC6, 0x00, 0x01, 0x00, 0x00][..]),
        ] {
            let mut output = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
            let written = write_packed_binary_header(byte_len, &mut output).unwrap();
            assert_eq!(&output[..written], expected);
            assert_eq!(packed_binary_header_len(byte_len), Some(expected.len()));
            assert_eq!(packed_binary_len(byte_len), Some(expected.len() + byte_len));
        }
        assert_eq!(
            write_packed_binary_header(256, &mut [0u8; 2]),
            Err(PackBinaryError::BufferTooShort)
        );
    }

    #[test]
    fn response_fits_packet_splits_at_the_mdu_and_the_respond_cap_and_a_dead_link() {
        let link_id = LinkId::new([0x43; 16]);
        let engine = engine_with_an_active_link_at(link_id, 300);
        let mdu = link_mdu(300);
        let largest_packet = (mdu - RESPONSE_WIRE_OVERHEAD).min(MAX_RESPOND_DATA_LEN);
        assert!(
            engine.response_fits_packet(&link_id, &std::vec![0xBB; largest_packet]),
            "a response at the packet ceiling stays a packet",
        );
        assert!(
            !engine.response_fits_packet(&link_id, &std::vec![0xBB; largest_packet + 1]),
            "one byte past the ceiling upgrades to a resource",
        );
        assert!(
            !engine.response_fits_packet(&LinkId::new([0x99; 16]), b"anything"),
            "a response over a link that is not active never claims the packet rung",
        );
    }

    #[test]
    fn request_fits_packet_requires_both_the_mdu_and_the_request_cap() {
        let link_id = LinkId::new([0x43; 16]);
        let engine = engine_with_an_active_link_at(link_id, 300);
        let largest_packet = link_mdu(300) - REQUEST_WIRE_OVERHEAD;
        assert!(engine.request_fits_packet(&link_id, &std::vec![0xBB; largest_packet]));
        assert!(!engine.request_fits_packet(&link_id, &std::vec![0xBB; largest_packet + 1],));

        let wide_link = LinkId::new([0x44; 16]);
        let wide_engine = engine_with_an_active_link_at(wide_link, BROADCAST_MTU * 2);
        assert!(!wide_engine
            .request_fits_packet(&wide_link, &std::vec![0xBB; MAX_SEND_REQUEST_DATA_LEN + 1],));
        assert!(!engine.request_fits_packet(&LinkId::new([0x99; 16]), b"anything"));
    }

    #[test]
    fn a_packet_response_settles_with_response_too_large_past_its_limit() {
        use crate::engine::{SendRequestFailure, Settlement};
        use crate::routing::links::data::write_link_packet;
        use crate::routing::links::resources::receive::tests_support::{
            engine_with_active_link, feed, link_id, link_key, track_pending_request_with_limit,
        };
        use crate::units::ByteLimit;

        let mut receiver = engine_with_active_link();
        let request_id = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(3),
        );
        let mut plaintext = [0u8; 64];
        let plaintext_len = write_response_plaintext(
            &request_id,
            &[BIN_8, 4, b't', b'e', b's', b't'],
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::Response,
            &plaintext[..plaintext_len],
            &[0xD2; 16],
            &mut frame,
        )
        .unwrap();

        let capture = feed(&mut receiver, &frame[..wire_bytes], 2_000);
        assert_eq!(
            capture.settlements,
            std::vec![(
                CommandId(42),
                Settlement::SendRequest(Err(SendRequestFailure::ResponseTooLarge)),
            )],
        );
        assert!(!receiver.receipts.has_pending_request(request_id));
    }

    #[test]
    fn a_packet_response_at_its_limit_still_settles_successfully() {
        use crate::engine::Settlement;
        use crate::routing::links::data::write_link_packet;
        use crate::routing::links::resources::receive::tests_support::{
            engine_with_active_link, feed, link_id, link_key, track_pending_request_with_limit,
        };
        use crate::units::ByteLimit;

        let mut receiver = engine_with_active_link();
        let request_id = track_pending_request_with_limit(
            &mut receiver,
            CommandId(42),
            1_800,
            20_000,
            ByteLimit::Maximum(4),
        );
        let mut plaintext = [0u8; 64];
        let plaintext_len = write_response_plaintext(
            &request_id,
            &[BIN_8, 4, b't', b'e', b's', b't'],
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_bytes = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::Response,
            &plaintext[..plaintext_len],
            &[0xD2; 16],
            &mut frame,
        )
        .unwrap();

        let capture = feed(&mut receiver, &frame[..wire_bytes], 2_000);
        assert!(matches!(
            capture.settlements.as_slice(),
            [(CommandId(42), Settlement::SendRequest(Ok(_)))]
        ));
        assert!(!receiver.receipts.has_pending_request(request_id));
    }

    #[test]
    fn parse_response_plaintext_refuses_each_header_gate_independently() {
        let id = RequestId([0x7E; 16]);
        let mut valid = [0u8; RESPONSE_WIRE_OVERHEAD + 1];
        write_response_plaintext(&id, &[], &mut valid).unwrap();
        assert!(parse_response_plaintext(&valid).is_ok());
        assert_eq!(
            parse_response_plaintext(&valid[..RESPONSE_WIRE_OVERHEAD]).unwrap_err(),
            ResponsePlaintextError::Malformed,
        );
        for (index, wrong) in [(0, 0x93u8), (1, 0xCB), (2, 0x0F)] {
            let mut bytes = valid;
            bytes[index] = wrong;
            assert_eq!(
                parse_response_plaintext(&bytes).unwrap_err(),
                ResponsePlaintextError::Malformed,
                "byte {index} must be checked on its own",
            );
        }
    }
}
