//! The host-agnostic core of the AX.25-KISS interface: the sizing the read and write loops are
//! built around, the AX.25 UI header the link wraps every packet in, and the descriptor the engine
//! sees. The KISS framing lives once in [`kiss_framing`](crate::interfaces::kiss_framing) and the
//! serve loop in `prns-interfaces-tokio`'s `framed_stream`; the only thing unique to AX.25
//! is the header built here. Decode is the inverse and config-independent — strip
//! [`AX25_HEADER_SIZE`] bytes — so it has no function here; the interface does it inline.

use crate::interfaces::kiss_framing::{self, KissDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};

pub const READ_BUF_LEN: usize = 256;
/// RNS `AX25KISSInterface.BITRATE_GUESS` — a 1200-baud TNC link, the conservative default for tiering.
pub const AX25_BITRATE_BPS: u32 = 1_200;
/// RNS `AX25KISSInterface.HW_MTU` — the Reticulum payload an AX.25 frame carries, before its header.
pub const AX25_HW_MTU: usize = 564;
/// RNS `AX25.HEADER_SIZE` — dest address (7) + source address (7) + control (1) + PID (1).
pub const AX25_HEADER_SIZE: usize = 16;
/// The deframer's payload ceiling: the AX.25 header plus the Reticulum payload (and access tag) a
/// single KISS frame carries on this link.
pub const AX25_FRAME_LEN: usize =
    AX25_HEADER_SIZE + AX25_HW_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = kiss_framing::max_encoded_len(AX25_FRAME_LEN);
pub type Decoder = KissDecoder<AX25_FRAME_LEN>;

/// The fixed AX.25 destination RNS addresses every frame to: callsign `APZRNS`, SSID 0.
const DEST_CALL: [u8; 6] = *b"APZRNS";
const DEST_SSID: u8 = 0;
/// AX.25 control field for an Unnumbered Information frame.
const CTRL_UI: u8 = 0x03;
/// AX.25 protocol id: no layer 3.
const PID_NOLAYER3: u8 = 0xF0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ax25AddressError {
    /// A callsign must be 3 to 6 characters.
    CallsignLength,
    /// A callsign must be ASCII.
    CallsignNotAscii,
    /// An SSID must be 0 to 15.
    SsidOutOfRange,
}

/// Encode one AX.25 address field (6 callsign bytes + an SSID byte) into `out`. Each callsign
/// character is shifted left one bit; positions past the callsign are padded with `0x20`, matching
/// RNS. `ssid_byte` is the already-composed SSID octet (the caller sets the command/reserved bits
/// and the end-of-address bit). `call` is assumed uppercase ASCII, 1..=6 bytes.
fn write_address(out: &mut [u8], call: &[u8], ssid_byte: u8) {
    for (i, slot) in out[..6].iter_mut().enumerate() {
        *slot = match call.get(i) {
            Some(&c) => c << 1,
            None => 0x20,
        };
    }
    out[6] = ssid_byte;
}

/// Build the 16-byte AX.25 UI header RNS prepends to every packet: the fixed `APZRNS-0` destination,
/// the configured source `callsign`/`ssid`, control = UI (`0x03`), PID = no-layer-3 (`0xF0`). The
/// callsign is uppercased; a short one is padded to six characters. The source SSID octet carries
/// the end-of-address bit (`0x01`); the destination octet does not.
pub fn build_header(callsign: &str, ssid: u8) -> Result<[u8; AX25_HEADER_SIZE], Ax25AddressError> {
    if ssid > 15 {
        return Err(Ax25AddressError::SsidOutOfRange);
    }
    let raw = callsign.as_bytes();
    if !(3..=6).contains(&raw.len()) {
        return Err(Ax25AddressError::CallsignLength);
    }
    let mut src = [0u8; 6];
    for (slot, &byte) in src.iter_mut().zip(raw) {
        if !byte.is_ascii() {
            return Err(Ax25AddressError::CallsignNotAscii);
        }
        *slot = byte.to_ascii_uppercase();
    }
    let src = &src[..raw.len()];

    let mut header = [0u8; AX25_HEADER_SIZE];
    // Destination address (bytes 0..7): command/reserved bits `0x60`, no end-of-address bit.
    write_address(&mut header[0..7], &DEST_CALL, 0x60 | (DEST_SSID << 1));
    // Source address (bytes 7..14): same `0x60` plus the end-of-address bit `0x01` on this, the
    // last address field.
    write_address(&mut header[7..14], src, 0x60 | (ssid << 1) | 0x01);
    header[14] = CTRL_UI;
    header[15] = PID_NOLAYER3;
    Ok(header)
}

pub fn descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: Some(AX25_HW_MTU),
        announce_rate_limit: None,
        bitrate_bps: Some(AX25_BITRATE_BPS),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_destination_is_apzrns_with_no_end_of_address_bit() {
        let header = build_header("N0CALL", 0).unwrap();
        // APZRNS, each character shifted left one bit.
        assert_eq!(&header[..6], &[0x82, 0xA0, 0xB4, 0xA4, 0x9C, 0xA6]);
        // Destination SSID octet: 0x60, SSID 0, end-of-address bit clear.
        assert_eq!(header[6], 0x60);
    }

    #[test]
    fn a_six_character_source_callsign_encodes_with_the_end_of_address_bit() {
        let header = build_header("N0CALL", 0).unwrap();
        assert_eq!(&header[7..13], &[0x9C, 0x60, 0x86, 0x82, 0x98, 0x98]);
        // Source SSID octet: 0x60, SSID 0, end-of-address bit set (0x01).
        assert_eq!(header[13], 0x61);
        assert_eq!(header[14], CTRL_UI);
        assert_eq!(header[15], PID_NOLAYER3);
    }

    #[test]
    fn a_short_callsign_is_padded_and_the_ssid_is_shifted_in() {
        let header = build_header("ABC", 5).unwrap();
        assert_eq!(&header[7..13], &[0x82, 0x84, 0x86, 0x20, 0x20, 0x20]);
        // 0x60 | (5 << 1) | 0x01 = 0x6B.
        assert_eq!(header[13], 0x6B);
    }

    #[test]
    fn a_lowercase_callsign_is_uppercased() {
        assert_eq!(build_header("n0call", 3), build_header("N0CALL", 3));
    }

    #[test]
    fn the_whole_header_round_trips_a_known_vector() {
        let header = build_header("N0CALL", 0).unwrap();
        assert_eq!(
            header,
            [
                0x82, 0xA0, 0xB4, 0xA4, 0x9C, 0xA6, 0x60, // APZRNS-0 destination
                0x9C, 0x60, 0x86, 0x82, 0x98, 0x98, 0x61, // N0CALL-0 source, end-of-address
                0x03, 0xF0, // control = UI, PID = no layer 3
            ]
        );
    }

    #[test]
    fn callsigns_and_ssids_are_validated() {
        assert_eq!(build_header("AB", 0), Err(Ax25AddressError::CallsignLength));
        assert_eq!(
            build_header("TOOLONG", 0),
            Err(Ax25AddressError::CallsignLength)
        );
        // "N0Å" is four bytes (Å is two), so it clears the length gate and trips the ASCII check.
        assert_eq!(
            build_header("N0Å", 0),
            Err(Ax25AddressError::CallsignNotAscii)
        );
        assert_eq!(
            build_header("N0CALL", 16),
            Err(Ax25AddressError::SsidOutOfRange)
        );
        assert!(build_header("N0CALL", 15).is_ok());
    }
}
