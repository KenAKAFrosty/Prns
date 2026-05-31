//! Platform-agnostic part of the LoRa interface: its routing descriptor, the
//! modulation profile both ends must agree on, and RNode's one-byte on-air
//! link-header codec.
//!
//! We speak RNode's LoRa on air directly — the Personal Hopspot *is* the modem,
//! not a host driving one over KISS-serial. So the on-air frame is **not** the
//! raw Reticulum packet: RNode prepends a single link-header byte (a sequence
//! nibble plus flags) ahead of the payload and strips it on receive
//! ([RNode_Firmware 1.86, `Framing.h`](https://github.com/markqvist/RNode_Firmware/blob/1.86/Framing.h#L60-L63)).
//! This module mints and parses that byte; the embassy shell owns the SX1262 and
//! the modulation.

use crate::interfaces::{
    Capabilities, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind,
};

/// RNode's on-air link header is one byte (`HEADER_L` in the firmware).
pub const LORA_HEADER_LEN: usize = 1;

/// The largest single LoRa frame RNode emits — explicit-header LoRa caps one
/// frame at 255 bytes (RNode `SINGLE_MTU`). A packet larger than
/// [`LORA_SINGLE_FRAME_PAYLOAD_MAX`] is split across frames by RNode; we don't
/// reassemble splits yet (announces — the first traffic we carry — are far
/// smaller), so this module frames single packets and *flags* received split
/// fragments for the worker to drop until reassembly lands.
pub const LORA_SINGLE_FRAME_MAX: usize = 255;

/// The most payload one on-air frame carries: the frame cap minus the header.
pub const LORA_SINGLE_FRAME_PAYLOAD_MAX: usize = LORA_SINGLE_FRAME_MAX - LORA_HEADER_LEN;

// RNode header-byte layout (Framing.h): high nibble = sequence, low nibble =
// flags; bit 0 of the flags marks a fragment of a split (multi-frame) packet.
const HEADER_SEQUENCE_NIBBLE: u8 = 0xF0;
const HEADER_FLAG_SPLIT: u8 = 0x01;

/// RNode's hardcoded LoRa sync word (every band) — the classic "private"
/// network sync ([RNode `sx126x.cpp`](https://github.com/markqvist/RNode_Firmware/blob/1.86/sx126x.cpp)).
/// lora-phy realizes this by constructing `LoRa` with `enable_public_network = false`.
pub const RNODE_LORA_SYNC_WORD: u16 = 0x1424;

/// The modulation both endpoints must match for RNode-compatible interop.
///
/// RNode firmware keeps no fixed modulation — the driving host sets frequency /
/// bandwidth / spreading factor / coding rate at runtime — so this is *our*
/// chosen profile, not an RNode constant. The preamble length, however, *is*
/// RNode's firmware default (18 symbols).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoRaModulation {
    pub frequency_hz: u32,
    pub bandwidth_hz: u32,
    pub spreading_factor: u8,
    /// Denominator of the 4/n coding rate (`5..=8` → 4/5 .. 4/8).
    pub coding_rate_denominator: u8,
    pub preamble_symbols: u16,
}

/// A standard 915 MHz (US ISM) Reticulum LoRa profile: BW 125 kHz, SF8, CR 4:5,
/// RNode's default 18-symbol preamble. Both the Hopspot and the peer RNode must
/// be configured to these exact values to hear each other; change them in one
/// place here if we retune.
pub const DEFAULT_915_LORA_PROFILE: LoRaModulation = LoRaModulation {
    frequency_hz: 915_000_000,
    bandwidth_hz: 125_000,
    spreading_factor: 8,
    coding_rate_denominator: 5,
    preamble_symbols: 18,
};

/// A parsed inbound on-air frame: the sequence tag and split flag read from
/// RNode's header byte, and the Reticulum-packet payload that followed it.
#[derive(Debug, PartialEq, Eq)]
pub struct AirFrame<'a> {
    /// The header's sequence value (high nibble, e.g. `0x30`). Ties the
    /// fragments of a split packet together; for a single frame it's an opaque tag.
    pub sequence: u8,
    /// RNode's split flag: this frame is one fragment of a packet that exceeded
    /// a single LoRa frame. We don't reassemble yet, so the worker drops these.
    pub is_split_fragment: bool,
    /// The Reticulum packet bytes carried after the header.
    pub payload: &'a [u8],
}

/// Why an outbound packet couldn't be framed into a single on-air LoRa frame.
#[derive(Debug, PartialEq, Eq)]
pub enum AirFrameError {
    /// The packet is larger than one LoRa frame can carry; multi-frame split is
    /// not implemented yet (deferred — announces fit a single frame).
    PayloadExceedsSingleFrame,
    /// The supplied output buffer can't hold the header byte plus the payload.
    OutputBufferTooSmall,
}

/// Frame `payload` for a single on-air transmission: RNode's one-byte link
/// header (sequence from `sequence_entropy`'s high nibble, split flag clear)
/// followed by the raw packet, written into `out`. Returns the framed length.
///
/// `sequence_entropy` is any byte; only its high nibble is used, mirroring
/// RNode's `random(256) & 0xF0`. The worker draws it from the engine's per-cycle
/// entropy so fragments would share a tag once split lands.
pub fn encode_air_frame(
    payload: &[u8],
    sequence_entropy: u8,
    out: &mut [u8],
) -> Result<usize, AirFrameError> {
    if payload.len() > LORA_SINGLE_FRAME_PAYLOAD_MAX {
        return Err(AirFrameError::PayloadExceedsSingleFrame);
    }
    if out.len() < LORA_HEADER_LEN + payload.len() {
        return Err(AirFrameError::OutputBufferTooSmall);
    }
    // Single frame → sequence in the high nibble, split flag (and all other
    // flags) clear.
    out[0] = sequence_entropy & HEADER_SEQUENCE_NIBBLE;
    out[LORA_HEADER_LEN..LORA_HEADER_LEN + payload.len()].copy_from_slice(payload);
    Ok(LORA_HEADER_LEN + payload.len())
}

/// Parse a received on-air frame: split RNode's header byte from the payload.
/// Returns `None` for an empty frame (no header byte to read).
pub fn decode_air_frame(frame: &[u8]) -> Option<AirFrame<'_>> {
    let (&header, payload) = frame.split_first()?;
    Some(AirFrame {
        sequence: header & HEADER_SEQUENCE_NIBBLE,
        is_split_fragment: header & HEADER_FLAG_SPLIT != 0,
        payload,
    })
}

/// The routing facts a LoRa interface registers: a shared half-duplex broadcast
/// medium where every neighbor hears every transmission and the node repeats
/// into it, participating fully in transport. Reported `Connected` once the
/// radio is initialized; a broadcast medium has no per-peer link state, so
/// liveness afterward is the worker's
/// [`health`](crate::interfaces::InterfaceWorker::health).
pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            // Every neighbor hears every transmission, so a LoRa node rebroadcasts
            // announces back into the same air — unlike a point-to-point cable.
            repeats: true,
        },
        mode: InterfaceMode::Full,
        medium: MediumKind::SharedHalfDuplex,
        state: ConnectionState::Connected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_single_frame_with_sequence_and_no_split_flag() {
        let payload = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut out = [0u8; 8];
        let n = encode_air_frame(&payload, 0x37, &mut out).unwrap();
        assert_eq!(n, 5);
        // header = high nibble of 0x37 = 0x30, split flag clear.
        assert_eq!(out[0], 0x30);
        assert_eq!(&out[1..5], &payload);
    }

    #[test]
    fn round_trips_through_decode() {
        let payload = [1u8, 2, 3, 0x7E, 0xFF, 0];
        let mut out = [0u8; 16];
        let n = encode_air_frame(&payload, 0xA0, &mut out).unwrap();
        let parsed = decode_air_frame(&out[..n]).unwrap();
        assert_eq!(parsed.sequence, 0xA0);
        assert!(!parsed.is_split_fragment);
        assert_eq!(parsed.payload, &payload);
    }

    #[test]
    fn decode_flags_a_split_fragment() {
        // header: sequence 0x50 + the split flag.
        let frame = [0x50 | HEADER_FLAG_SPLIT, 0xAA, 0xBB];
        let parsed = decode_air_frame(&frame).unwrap();
        assert_eq!(parsed.sequence, 0x50);
        assert!(parsed.is_split_fragment);
        assert_eq!(parsed.payload, &[0xAA, 0xBB]);
    }

    #[test]
    fn decode_empty_frame_is_none() {
        assert!(decode_air_frame(&[]).is_none());
    }

    #[test]
    fn rejects_payload_larger_than_a_single_frame() {
        let payload = [0u8; LORA_SINGLE_FRAME_PAYLOAD_MAX + 1];
        let mut out = [0u8; 300];
        assert_eq!(
            encode_air_frame(&payload, 0, &mut out),
            Err(AirFrameError::PayloadExceedsSingleFrame)
        );
    }

    #[test]
    fn rejects_output_buffer_too_small() {
        let payload = [1u8, 2, 3];
        let mut out = [0u8; 3]; // needs 4 (header + 3)
        assert_eq!(
            encode_air_frame(&payload, 0, &mut out),
            Err(AirFrameError::OutputBufferTooSmall)
        );
    }

    #[test]
    fn max_single_frame_payload_fills_the_frame() {
        let payload = [0x5Au8; LORA_SINGLE_FRAME_PAYLOAD_MAX];
        let mut out = [0u8; LORA_SINGLE_FRAME_MAX];
        let n = encode_air_frame(&payload, 0xF0, &mut out).unwrap();
        assert_eq!(n, LORA_SINGLE_FRAME_MAX);
        let parsed = decode_air_frame(&out[..n]).unwrap();
        assert_eq!(parsed.payload.len(), LORA_SINGLE_FRAME_PAYLOAD_MAX);
    }

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(InterfaceId::new([0x5C; 16]));
        assert!(matches!(d.medium, MediumKind::SharedHalfDuplex));
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert!(matches!(d.state, ConnectionState::Connected));
        assert!(d.capabilities.repeats);
        assert!(d.capabilities.receives && d.capabilities.transmits && d.capabilities.forwards);
    }
}
