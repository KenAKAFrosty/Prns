//! RNS 1.3.1 request/response over a link — `Link.request` (context 0x09) and
//! its answer (context 0x0A), both sealed under the session key. A request is
//! msgpack `[time, truncated_hash(path), data]`; its id is the first sixteen
//! bytes of the request packet's hash, and the response names that id back:
//! msgpack `[request_id, data]`. The `data` field crosses this engine as raw
//! msgpack value bytes, never interpreted — the app packs and unpacks whatever
//! the reference's apps would, byte for byte. Payloads past the link MDU are
//! Resource territory and refused here.

use crate::engine::commands::{
    CommandId, CommandOutcome, Respond, RespondError, SendRequest, SendRequestError,
    MAX_RESPOND_DATA_LEN, MAX_SEND_REQUEST_DATA_LEN,
};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::IdentitySigningPublicKey;
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{CulledReceipt, OutstandingReceipt, ReceiptKind};
use crate::routing::links::data::{
    link_mdu, LINK_TRAFFIC_TIMEOUT_FACTOR, LINK_TRAFFIC_TIMEOUT_MIN_MS,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::request_handlers::RequestPathHash;
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, TRUNCATED_HASH_BYTE_LEN,
};

/// RNS 1.3.1 `Resource.RESPONSE_MAX_GRACE_TIME` (10 s) × 1.125, the flat term
/// in a request's default timeout: `rtt × traffic_timeout_factor + 11.25 s`.
pub const REQUEST_RESPONSE_GRACE_MS: u64 = 11_250;

/// msgpack `fixarray(3)` ‖ `float64` time ‖ `bin8(16)` path hash, before data.
pub const REQUEST_WIRE_OVERHEAD: usize = 1 + 9 + 2 + TRUNCATED_HASH_BYTE_LEN;
/// The largest wrapped plaintext either verb stages before sealing: the msgpack envelope
/// plus the largest data it admits — both verbs' data caps were derived from the
/// single-packet link MDU, so the two sides land on the same figure by construction.
pub const WRAPPED_PLAINTEXT_CAP: usize = {
    let request = REQUEST_WIRE_OVERHEAD + MAX_SEND_REQUEST_DATA_LEN;
    let response = RESPONSE_WIRE_OVERHEAD + MAX_RESPOND_DATA_LEN;
    if request > response {
        request
    } else {
        response
    }
};
/// msgpack `fixarray(2)` ‖ `bin8(16)` request id, before data.
pub const RESPONSE_WIRE_OVERHEAD: usize = 1 + 2 + TRUNCATED_HASH_BYTE_LEN;

const FIXARRAY_3: u8 = 0x93;
const FIXARRAY_2: u8 = 0x92;
const FLOAT_64: u8 = 0xCB;
const BIN_8: u8 = 0xC4;
const NIL: u8 = 0xC0;

/// The truncated hash of the request packet — RNS 1.3.1
/// `packet.getTruncatedHash()` — naming the request in its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub [u8; TRUNCATED_HASH_BYTE_LEN]);

impl RequestId {
    #[must_use]
    pub fn of_packet(packet_hash: &PacketHash) -> Self {
        let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
        id.copy_from_slice(&packet_hash.as_bytes()[..TRUNCATED_HASH_BYTE_LEN]);
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

/// `umsgpack.packb([time.time(), request_path_hash, data])` — the float lives
/// and dies inside this codec; the engine clock stays u64 millis. Empty `data`
/// packs the reference's `None` as nil.
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

/// The responder's read — hostile floats saturate the way the LRRTT parse
/// does, and anything not shaped like the reference's three-element pack is
/// refused.
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

/// `umsgpack.packb([request_id, response])`.
pub fn write_response_plaintext(
    request_id: &RequestId,
    data: &[u8],
    buf: &mut [u8],
) -> Result<usize, ResponsePlaintextError> {
    let data_len = if data.is_empty() { 1 } else { data.len() };
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
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub request_id: RequestId,
    pub culled: Option<CulledReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RespondDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRequestWriteError {
    LinkVanished,
    PayloadTooLong,
    BufferTooShort,
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
        destination: DestinationHash::new(*link_id.as_bytes()),
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
                error: SendRequestError::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::SendRequestRejected {
                    id,
                    error: SendRequestError::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => CommandOutcome::OwesSendRequest { id, request },
        }
    }

    pub fn ingest_respond(&self, id: CommandId, respond: Respond) -> CommandOutcome {
        match self.links.phase_for(&respond.link_id) {
            None => CommandOutcome::RespondRejected {
                id,
                error: RespondError::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::RespondRejected {
                    id,
                    error: RespondError::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => CommandOutcome::OwesRespond { id, respond },
        }
    }

    /// Seal a request and book its pending row: the request settles when a
    /// response names its id back, or times out at
    /// `rtt × 6 + `[`REQUEST_RESPONSE_GRACE_MS`] — RNS 1.3.1 `Link.request`'s
    /// default timeout.
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
            rtt_ms,
            peer_signing,
            ..
        }) = self.links.phase_for(&request.link_id)
        else {
            return Err(LinkRequestWriteError::LinkVanished);
        };
        let fire_on = *attached_interface;
        let peer_signing = *peer_signing;
        let timeout_ms = rtt_ms
            .saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR)
            .max(LINK_TRAFFIC_TIMEOUT_MIN_MS)
            .saturating_add(REQUEST_RESPONSE_GRACE_MS);

        let mut plaintext = [0u8; WRAPPED_PLAINTEXT_CAP];
        let plain_len =
            write_request_plaintext(now, &request.path_hash, &request.data, &mut plaintext)
                .map_err(|_| LinkRequestWriteError::BufferTooShort)?;
        if plain_len > link_mdu(*mtu) {
            return Err(LinkRequestWriteError::PayloadTooLong);
        }
        let (header_len, wire_len) = seal_link_frame(
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
            &DestinationHash::new(*request.link_id.as_bytes()),
            WireContext::Request,
            &buf[header_len..wire_len],
        );
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: id,
            kind: ReceiptKind::SendRequest,
            peer_signing_key: IdentitySigningPublicKey::new(peer_signing),
            sent_at: now,
            timeout_at: InstantMillis(now.0.saturating_add(timeout_ms)),
        });

        Ok(SendRequestDispatch {
            wire_len,
            fire_on,
            request_id: RequestId::of_packet(&packet_hash),
            culled,
        })
    }

    /// Seal a response naming the request id back. Fire-and-forget: the
    /// reference sends its response packet and moves on.
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
        let mut plaintext = [0u8; WRAPPED_PLAINTEXT_CAP];
        let plain_len =
            write_response_plaintext(&respond.request_id, &respond.data, &mut plaintext)
                .map_err(|_| LinkRequestWriteError::BufferTooShort)?;
        if plain_len > link_mdu(*mtu) {
            return Err(LinkRequestWriteError::PayloadTooLong);
        }
        let (_, wire_len) = seal_link_frame(
            &respond.link_id,
            key,
            WireContext::Response,
            &plaintext[..plain_len],
            iv,
            buf,
        )
        .ok_or(LinkRequestWriteError::BufferTooShort)?;
        Ok(RespondDispatch {
            wire_len,
            fire_on: *attached_interface,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH_HASH: RequestPathHash = RequestPathHash::new([0x5A; 16]);

    #[test]
    fn the_request_pack_is_byte_identical_to_umsgpack() {
        // umsgpack.packb([2.5, bytes([0x5A]*16), b"\xA3abc"]) — fixarray(3),
        // float64 2.5, bin8(16), then the data bytes verbatim (here a fixstr).
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
        // One byte below the minimum length is malformed; the exact minimum parses.
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
                100,
                mtu,
                InterfaceId::new([0xEE; 16]),
                InstantMillis(1_000),
                Ed25519PublicKey([0x5A; 32]),
            )
            .unwrap();
        engine
    }

    #[test]
    fn write_commanded_send_request_admits_an_exact_mdu_payload_and_refuses_one_byte_past() {
        use crate::engine::commands::SendRequestData;
        let link_id = LinkId::new([0x42; 16]);
        let mdu = link_mdu(300);
        let send = |data_len: usize| {
            let request = SendRequest {
                link_id,
                path_hash: RequestPathHash::of("/q"),
                data: SendRequestData::from_slice(&std::vec![0xAA; data_len]).unwrap(),
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
    fn write_commanded_respond_admits_an_exact_mdu_payload_and_refuses_one_byte_past() {
        use crate::engine::commands::RespondData;
        let link_id = LinkId::new([0x43; 16]);
        let mdu = link_mdu(300);
        let respond = |data_len: usize| {
            let respond = Respond {
                link_id,
                request_id: RequestId([0x7E; 16]),
                data: RespondData::from_slice(&std::vec![0xBB; data_len]).unwrap(),
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
