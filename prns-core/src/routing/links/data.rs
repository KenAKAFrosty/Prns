use crate::crypto::TOKEN_OVERHEAD;
use crate::engine::{CommandId, CommandOutcome, SendToLink, SendToLinkRejection};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::IdentitySigningPublicKey;
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{CulledReceipt, OutstandingReceipt, ReceiptKind};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::{LinkId, LinkKey};
use crate::storage::StorageLayout;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader, BROADCAST_MTU, HEADER_MIN_LEN, IFAC_MIN_LEN,
};

/// RNS 1.3.5 `Link.TRAFFIC_TIMEOUT_FACTOR` / `TRAFFIC_TIMEOUT_MIN_MS`: how long
/// a link send waits for its proof before giving up.
pub const LINK_TRAFFIC_TIMEOUT_FACTOR: u64 = 6;
pub const LINK_TRAFFIC_TIMEOUT_MIN_MS: u64 = 5;

/// RNS 1.3.5 `Link.update_mdu`: the most plaintext one link data packet can
/// carry: the link MTU less the type-1 header, minimum IFAC, and token
/// overhead, floored to a whole AES block, minus one pad byte.
pub const fn link_mdu(mtu: usize) -> usize {
    ((mtu - IFAC_MIN_LEN - HEADER_MIN_LEN - TOKEN_OVERHEAD) / 16) * 16 - 1
}

pub const LINK_MDU: usize = link_mdu(BROADCAST_MTU);

pub const fn link_data_frame_ceiling(plaintext_len: usize) -> usize {
    HEADER_MIN_LEN + IFAC_MIN_LEN + TOKEN_OVERHEAD + ((plaintext_len / 16) + 1) * 16
}

pub const fn link_raw_frame_ceiling(payload_len: usize) -> usize {
    HEADER_MIN_LEN + IFAC_MIN_LEN + payload_len
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDataError {
    PayloadTooLong,
    BufferTooShort,
}

pub fn write_link_data(
    link_id: &LinkId,
    link_key: &LinkKey,
    mtu: usize,
    plaintext: &[u8],
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Result<usize, LinkDataError> {
    write_link_packet(
        link_id,
        link_key,
        mtu,
        WireContext::None,
        plaintext,
        iv,
        buf,
    )
}

/// [`write_link_data`] with the context byte the resource family dictates: an
/// advertisement, a part request, a hashmap update.
pub fn write_link_packet(
    link_id: &LinkId,
    link_key: &LinkKey,
    mtu: usize,
    context: WireContext,
    plaintext: &[u8],
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Result<usize, LinkDataError> {
    if plaintext.len() > link_mdu(mtu) {
        return Err(LinkDataError::PayloadTooLong);
    }
    let header = link_packet_header(link_id, PacketType::Data, context);
    let header_len = header
        .write(buf)
        .map_err(|_| LinkDataError::BufferTooShort)?;
    let sealed = link_key
        .seal(iv, plaintext, &mut buf[header_len..])
        .map_err(|_| LinkDataError::BufferTooShort)?;
    Ok(header_len + sealed)
}

/// A link packet whose payload rides exactly as given. No token around it.
/// What RNS 1.3.5 `Packet.pack` does for context `RESOURCE` (parts are
/// slices of an already-sealed stream) and `RESOURCE_PRF` (the proof is a
/// bare hash pair on a PROOF-type packet).
pub fn write_link_raw_packet(
    link_id: &LinkId,
    packet_type: PacketType,
    context: WireContext,
    mtu: usize,
    payload: &[u8],
    buf: &mut [u8],
) -> Result<usize, LinkDataError> {
    if payload.len() > mtu - HEADER_MIN_LEN - IFAC_MIN_LEN {
        return Err(LinkDataError::PayloadTooLong);
    }
    let header = link_packet_header(link_id, packet_type, context);
    let header_len = header
        .write(buf)
        .map_err(|_| LinkDataError::BufferTooShort)?;
    let end = header_len + payload.len();
    if buf.len() < end {
        return Err(LinkDataError::BufferTooShort);
    }
    buf[header_len..end].copy_from_slice(payload);
    Ok(end)
}

fn link_packet_header(
    link_id: &LinkId,
    packet_type: PacketType,
    context: WireContext,
) -> WirePacketHeader {
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type,
        hops: 0,
        transport_id: None,
        destination: DestinationHash::new(*link_id.as_bytes()),
        context,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendToLinkDispatch {
    pub wire_len: usize,
    pub fire_on: InterfaceId,
    pub culled: Option<CulledReceipt>,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_send_to_link(&self, id: CommandId, send: SendToLink) -> CommandOutcome {
        match self.links.phase_for(&send.link_id) {
            None => CommandOutcome::SendToLinkRejected {
                id,
                rejection: SendToLinkRejection::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::SendToLinkRejected {
                    id,
                    rejection: SendToLinkRejection::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => CommandOutcome::OwesSendToLink { id, send },
        }
    }

    /// Seal `send`'s payload under the link's session key, bounded by the
    /// link's negotiated MDU, framed directly into `buf` and owed to the
    /// interface the link rides — RNS 1.3.5 `Packet(link, data).send()`. The
    /// send is tracked as an outstanding receipt: it settles when the
    /// responder's proof validates, or times out at the link's traffic
    /// deadline (`max(rtt × 6, 5 ms)`).
    pub fn write_commanded_send_to_link(
        &mut self,
        id: CommandId,
        send: &SendToLink,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<SendToLinkDispatch, SendToLinkWriteError> {
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt,
            peer_signing,
            ..
        }) = self.links.phase_for(&send.link_id)
        else {
            return Err(SendToLinkWriteError::LinkVanished);
        };
        let fire_on = *attached_interface;
        let peer_signing = *peer_signing;
        let traffic_timeout_ms = rtt
            .millis()
            .saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR)
            .max(LINK_TRAFFIC_TIMEOUT_MIN_MS);
        let wire_len = write_link_data(&send.link_id, key, *mtu, &send.payload, iv, buf)
            .map_err(SendToLinkWriteError::Frame)?;

        let packet_hash = PacketHash::of_data_fields(
            DestinationType::Link,
            &DestinationHash::new(*send.link_id.as_bytes()),
            WireContext::None,
            &buf[HEADER_MIN_LEN..wire_len],
        );
        let culled = self.receipts.track(OutstandingReceipt {
            packet_hash,
            command_id: id,
            kind: ReceiptKind::SendToLink,
            peer_signing_key: IdentitySigningPublicKey::new(peer_signing),
            sent_at: now,
            timeout_at: InstantMillis(now.0.saturating_add(traffic_timeout_ms)),
        });

        Ok(SendToLinkDispatch {
            wire_len,
            fire_on,
            culled,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToLinkWriteError {
    LinkVanished,
    Frame(LinkDataError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }
    fn a16(s: &str) -> [u8; 16] {
        hx(s).try_into().expect("16 bytes")
    }
    fn a32(s: &str) -> [u8; 32] {
        hx(s).try_into().expect("32 bytes")
    }

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    const CIPHER_IV: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
    const PLAINTEXT: &[u8] = b"link layer rides the same token!";
    const LINK_DATA_PACKET: &str = "0c00000102030405060708090a0b0c0d0e0f00\
                                    a1a2a3a4a5a6a7a8a9aaabacadaeafb012a31f7217fde987fbb8bab1ef73d3b3\
                                    b63557757d0c3adea6b0e94e9d27f23ba732763cc4ed566de7c915bafe3e5467\
                                    99a834e0e6579c62ccb6da661641040a56430127964af6eafdae462cd79e8ff0";

    fn link_key() -> LinkKey {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new(a32(INITIATOR_SCALAR)),
            &X25519PublicKey(a32(RESPONDER_PUBLIC)),
        );
        LinkKey::derive(&LinkId::new(a16(LINK_ID)), &shared)
    }

    #[test]
    fn the_link_mdu_matches_the_reference_arithmetic() {
        assert_eq!(LINK_MDU, 431);
        assert_eq!(link_mdu(1_064), 991);
    }

    #[test]
    fn write_link_data_frames_the_reference_token_behind_the_data_header() {
        let mut buf = [0u8; BROADCAST_MTU];
        let n = write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &link_key(),
            BROADCAST_MTU,
            PLAINTEXT,
            &a16(CIPHER_IV),
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &hx(LINK_DATA_PACKET)[..]);
    }

    #[test]
    fn a_sealed_frame_opens_in_place_to_the_plaintext() {
        let key = link_key();
        let mut buf = [0u8; BROADCAST_MTU];
        let n = write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &key,
            BROADCAST_MTU,
            PLAINTEXT,
            &a16(CIPHER_IV),
            &mut buf,
        )
        .unwrap();
        let (header, _) = WirePacketHeader::parse(&buf[..n]).unwrap();
        assert_eq!(header.destination, DestinationHash::new(a16(LINK_ID)));
        assert_eq!(header.context, WireContext::None);

        let opened = key.open_in_place(&mut buf[HEADER_MIN_LEN..n]).unwrap();
        assert_eq!(opened, PLAINTEXT);
    }

    #[test]
    fn a_payload_past_the_link_mdu_is_refused() {
        let mut buf = [0u8; 1_024];
        assert_eq!(
            write_link_data(
                &LinkId::new(a16(LINK_ID)),
                &link_key(),
                BROADCAST_MTU,
                &[0u8; LINK_MDU + 1],
                &a16(CIPHER_IV),
                &mut buf,
            ),
            Err(LinkDataError::PayloadTooLong),
        );
        assert!(write_link_data(
            &LinkId::new(a16(LINK_ID)),
            &link_key(),
            BROADCAST_MTU,
            &[0u8; LINK_MDU],
            &a16(CIPHER_IV),
            &mut buf,
        )
        .is_ok());
    }
}
