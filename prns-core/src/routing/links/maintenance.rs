use crate::engine::EngineState;
use crate::engine::{CloseLink, CloseLinkRejection, CommandId, CommandOutcome};
use crate::interfaces::InterfaceId;
use crate::routing::links::channel::table::ChannelTable;
use crate::routing::links::table::LinkPhase;
use crate::routing::links::{LinkId, LinkKey};
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{
    ContextFlag, DestinationType, IfacFlag, PacketType, PropagationType, WireContext, WireError,
    WirePacketHeader,
};

pub const KEEPALIVE_REQUEST: u8 = 0xFF;
pub const KEEPALIVE_ECHO: u8 = 0xFE;

pub const KEEPALIVE_MAX_MS: u64 = 360_000;
pub const KEEPALIVE_MAX_RTT_MS: u64 = 1_750;
pub const KEEPALIVE_MIN_MS: u64 = 5_000;

pub fn keepalive_ms_from(rtt: RttMillis) -> u64 {
    (rtt.millis().saturating_mul(KEEPALIVE_MAX_MS) / KEEPALIVE_MAX_RTT_MS)
        .clamp(KEEPALIVE_MIN_MS, KEEPALIVE_MAX_MS)
}

pub const STALE_FACTOR: u64 = 2;

pub fn stale_ms_from(keepalive_ms: u64) -> u64 {
    keepalive_ms.saturating_mul(STALE_FACTOR)
}

pub const KEEPALIVE_TIMEOUT_FACTOR: u64 = 4;
pub const STALE_GRACE_MS: u64 = 5_000;

pub fn timeout_grace_ms_from(rtt: RttMillis) -> u64 {
    rtt.millis()
        .saturating_mul(KEEPALIVE_TIMEOUT_FACTOR)
        .saturating_add(STALE_GRACE_MS)
}

fn link_frame_header(link_id: &LinkId, context: WireContext) -> WirePacketHeader {
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link_id.to_address(),
        context,
    }
}

pub fn write_keepalive(link_id: &LinkId, byte: u8, buf: &mut [u8]) -> Result<usize, WireError> {
    let header_len = link_frame_header(link_id, WireContext::KeepAlive).write(buf)?;
    if buf.len() < header_len + 1 {
        return Err(WireError::BufferTooShort);
    }
    buf[header_len] = byte;
    Ok(header_len + 1)
}

pub fn write_link_close(
    link_id: &LinkId,
    link_key: &LinkKey,
    iv: &[u8; 16],
    buf: &mut [u8],
) -> Result<usize, WireError> {
    let header_len = link_frame_header(link_id, WireContext::LinkClose).write(buf)?;
    let sealed = link_key
        .seal(iv, link_id.as_bytes(), &mut buf[header_len..])
        .map_err(|_| WireError::BufferTooShort)?;
    Ok(header_len + sealed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkCloseDispatch {
    pub wire_bytes: usize,
    pub fire_on: Option<InterfaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteLinkCloseError {
    NoSuchLink,
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_close_link(&self, id: CommandId, close: CloseLink) -> CommandOutcome {
        match self.links.phase_for(&close.link_id) {
            None => CommandOutcome::CloseLinkRejected {
                id,
                rejection: CloseLinkRejection::NoSuchLink,
            },
            Some(LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }) => {
                CommandOutcome::CloseLinkRejected {
                    id,
                    rejection: CloseLinkRejection::LinkNotActive,
                }
            }
            Some(LinkPhase::Active { .. }) => CommandOutcome::OwesLinkClose { id, close },
        }
    }

    /// Seal the LINKCLOSE the way RNS 1.4.2 `Link.teardown` does (the link_id  encrypted under the session key) then forget the link; the dropped row zeroizes its key material.
    pub fn write_owed_link_close(
        &mut self,
        link_id: &LinkId,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<LinkCloseDispatch, WriteLinkCloseError> {
        let (key, fire_on) = match self.links.phase_for(link_id) {
            Some(LinkPhase::Active {
                key,
                attached_interface,
                ..
            }) => (key, Some(*attached_interface)),
            Some(LinkPhase::Handshake { key, .. }) => (key, None),
            Some(LinkPhase::Pending { .. }) | None => {
                return Err(WriteLinkCloseError::NoSuchLink);
            }
        };
        let wire_bytes =
            write_link_close(link_id, key, iv, buf).map_err(|_| WriteLinkCloseError::Serialize)?;
        self.reconcile_pending_link_route_evidence();
        self.links.remove(link_id);
        self.channels.close(link_id);
        self.pending_resource_offers.remove_link(link_id);
        self.incoming_assemblies.clear(link_id);
        self.outgoing_assemblies.clear(link_id);
        if let Some(interface) = fire_on {
            self.mark_interface_dirty(interface);
        }
        Ok(LinkCloseDispatch {
            wire_bytes,
            fire_on,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};

    fn bytes_from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn the_keepalive_cadence_matches_the_reference_law() {
        assert_eq!(keepalive_ms_from(RttMillis::new(0)), 5_000);
        assert_eq!(keepalive_ms_from(RttMillis::new(250)), 51_428);
        assert_eq!(keepalive_ms_from(RttMillis::new(1_750)), 360_000);
        assert_eq!(keepalive_ms_from(RttMillis::new(10_000)), 360_000);
        assert_eq!(stale_ms_from(51_428), 102_856);
        assert_eq!(timeout_grace_ms_from(RttMillis::new(0)), 5_000);
        assert_eq!(timeout_grace_ms_from(RttMillis::new(250)), 6_000);
        assert_eq!(timeout_grace_ms_from(RttMillis::new(2_000)), 13_000);
    }

    #[test]
    fn keepalive_frames_are_one_plaintext_byte() {
        let link_id = LinkId::new([
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
            0x0E, 0x0F,
        ]);
        let mut buf = [0u8; 64];
        let n = write_keepalive(&link_id, KEEPALIVE_REQUEST, &mut buf).unwrap();
        assert_eq!(
            &buf[..n],
            &bytes_from_hex("0c00000102030405060708090a0b0c0d0e0ffaff")[..],
        );
        let n = write_keepalive(&link_id, KEEPALIVE_ECHO, &mut buf).unwrap();
        assert_eq!(buf[n - 1], KEEPALIVE_ECHO);
    }

    #[test]
    fn write_keepalive_fills_an_exact_buffer_and_rejects_one_byte_short() {
        let link_id = LinkId::new([0x33; 16]);
        let exact = write_keepalive(&link_id, KEEPALIVE_REQUEST, &mut [0u8; 64]).unwrap();
        let mut fits = std::vec![0u8; exact];
        assert_eq!(
            write_keepalive(&link_id, KEEPALIVE_REQUEST, &mut fits),
            Ok(exact)
        );
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(
            write_keepalive(&link_id, KEEPALIVE_REQUEST, &mut short),
            Err(WireError::BufferTooShort)
        );
    }

    #[test]
    fn the_link_close_seals_the_link_id_under_the_session_key() {
        let link_id = LinkId::new([0x11; 16]);
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x55; 32]),
        );
        let key = LinkKey::derive(&link_id, &shared);
        let mut buf = [0u8; 128];
        let n = write_link_close(&link_id, &key, &[0xA1; 16], &mut buf).unwrap();
        assert_eq!(buf[18], 0xFC, "the context byte names the LINKCLOSE");
        let opened = key.open_in_place(&mut buf[19..n]).unwrap();
        assert_eq!(opened, link_id.as_bytes());
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn keepalive_for_any_rtt_stays_inside_the_reference_clamp() {
        let rtt_millis: u64 = kani::any();
        let keepalive_ms = keepalive_ms_from(RttMillis::new(rtt_millis));
        assert!(keepalive_ms >= 5_000);
        assert!(keepalive_ms <= 360_000);
    }

    #[kani::proof]
    fn stale_is_exactly_twice_any_clamped_keepalive() {
        let rtt_millis: u64 = kani::any();
        let keepalive_ms = keepalive_ms_from(RttMillis::new(rtt_millis));
        assert_eq!(stale_ms_from(keepalive_ms), keepalive_ms * 2);
    }

    #[kani::proof]
    fn the_grace_never_dips_below_the_stale_grace_floor() {
        let rtt_millis: u64 = kani::any();
        assert!(timeout_grace_ms_from(RttMillis::new(rtt_millis)) >= STALE_GRACE_MS);
    }
}
