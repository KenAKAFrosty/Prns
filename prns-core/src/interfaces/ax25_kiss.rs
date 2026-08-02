use crate::interfaces::kiss_framing::{self, KissDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const READ_BUF_LEN: usize = 256;
pub const AX25_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_200);
pub const AX25_HW_MTU: usize = 564;
pub const AX25_HEADER_SIZE: usize = 16;
pub const AX25_FRAME_LEN: usize = AX25_HEADER_SIZE + AX25_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = kiss_framing::max_encoded_len(AX25_FRAME_LEN);
pub type Decoder = KissDecoder<AX25_FRAME_LEN>;

const DEST_CALL: [u8; 6] = *b"APZRNS";
const DEST_SSID: u8 = 0;
const CTRL_UI: u8 = 0x03;
const PID_NOLAYER3: u8 = 0xF0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ax25AddressError {
    CallsignLength,
    CallsignNotAscii,
    SsidOutOfRange,
}

/// Each callsign character is shifted left one bit and unused positions are padded with `0x20`, matching RNS. `ssid_byte` is already composed with the command, reserved, and end-of-address bits.
fn write_address(out: &mut [u8], call: &[u8], ssid_byte: u8) {
    for (i, slot) in out[..6].iter_mut().enumerate() {
        *slot = match call.get(i) {
            Some(&c) => c << 1,
            None => 0x20,
        };
    }
    out[6] = ssid_byte;
}

/// RNS addresses every packet to `APZRNS-0`; only the source SSID octet carries the end-of-address bit.
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
    write_address(&mut header[0..7], &DEST_CALL, 0x60 | (DEST_SSID << 1));
    write_address(&mut header[7..14], src, 0x60 | (ssid << 1) | 0x01);
    header[14] = CTRL_UI;
    header[15] = PID_NOLAYER3;
    Ok(header)
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    bitrate: AX25_BITRATE_BPS,
    mtu: MtuPolicy::fixed(AX25_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_destination_is_apzrns_with_no_end_of_address_bit() {
        let header = build_header("N0CALL", 0).unwrap();
        assert_eq!(&header[..6], &[0x82, 0xA0, 0xB4, 0xA4, 0x9C, 0xA6]);
        assert_eq!(header[6], 0x60);
    }

    #[test]
    fn a_six_character_source_callsign_encodes_with_the_end_of_address_bit() {
        let header = build_header("N0CALL", 0).unwrap();
        assert_eq!(&header[7..13], &[0x9C, 0x60, 0x86, 0x82, 0x98, 0x98]);
        assert_eq!(header[13], 0x61);
        assert_eq!(header[14], CTRL_UI);
        assert_eq!(header[15], PID_NOLAYER3);
    }

    #[test]
    fn a_short_callsign_is_padded_and_the_ssid_is_shifted_in() {
        let header = build_header("ABC", 5).unwrap();
        assert_eq!(&header[7..13], &[0x82, 0x84, 0x86, 0x20, 0x20, 0x20]);
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
