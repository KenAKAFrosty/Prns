//! Platform-agnostic part of the LoRa interface: its routing descriptor, the
//! modulation profile both ends must agree on, and RNode's on-air link-header
//! codec — framing (with multi-frame split) on transmit, reassembly on receive.
//!
//! We speak RNode's LoRa on air directly — the Personal Hopspot *is* the modem,
//! not a host driving one over KISS-serial. So the on-air frame is **not** the
//! raw Reticulum packet: RNode prepends a one-byte link header (a sequence nibble
//! plus a split flag). A packet that fits one LoRa frame is sent whole; a larger
//! one (up to [`LORA_MAX_PAYLOAD`]) is split across two frames sharing one header,
//! and the receiver reassembles them — see
//! [RNode_Firmware 1.86 `transmit`](https://github.com/markqvist/RNode_Firmware/blob/1.86/RNode_Firmware.ino#L716-L760)
//! / [`receive_callback`](https://github.com/markqvist/RNode_Firmware/blob/1.86/RNode_Firmware.ino#L359-L450).
//! This module mints, splits, parses, and reassembles those frames; the embassy
//! worker owns the SX1262 and the modulation.

use heapless::Vec as HeaplessVec;

use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind, TransitCapability,
};

/// RNode's on-air link header is one byte (`HEADER_L` in the firmware).
pub const LORA_HEADER_LEN: usize = 1;

/// The largest single LoRa frame RNode emits — explicit-header LoRa caps one
/// frame at 255 bytes (RNode `SINGLE_MTU`).
pub const LORA_SINGLE_FRAME_MAX: usize = 255;

pub const LORA_SINGLE_FRAME_PAYLOAD_MAX: usize = LORA_SINGLE_FRAME_MAX - LORA_HEADER_LEN;

/// The most payload we carry across a single packet's frames — RNode's `MTU`
/// (508): two frames of [`LORA_SINGLE_FRAME_PAYLOAD_MAX`]. (Reticulum's own MTU
/// is 500, so real packets always fit; this is the hard ceiling.)
pub const LORA_MAX_PAYLOAD: usize = 2 * LORA_SINGLE_FRAME_PAYLOAD_MAX;

// RNode header-byte layout (Framing.h): high nibble = sequence, low nibble =
// flags; bit 0 of the flags marks a frame that is part of a split (multi-frame)
// packet — set on *every* frame of the split, cleared on a whole-in-one-frame packet.
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
    pub coding_rate_denominator: u8,
    pub preamble_symbols: u16,
}

pub const DEFAULT_915_LORA_PROFILE: LoRaModulation = LoRaModulation {
    frequency_hz: 915_000_000,
    bandwidth_hz: 125_000,
    spreading_factor: 8,
    coding_rate_denominator: 5,
    preamble_symbols: 18,
};

#[derive(Debug, PartialEq, Eq)]
pub struct AirFrame<'a> {
    pub sequence: u8,
    pub is_split_fragment: bool,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub enum AirFrameError {
    PayloadExceedsMax,
    OutputBufferTooSmall,
}

pub const fn air_frame_count(payload_len: usize) -> usize {
    if payload_len <= LORA_SINGLE_FRAME_PAYLOAD_MAX {
        1
    } else {
        2
    }
}

/// The caller transmits frames `0..air_frame_count(payload.len())`, all with the
/// same `sequence_entropy` so the receiver reassembles them — mirroring RNode,
/// which picks one `random(256) & 0xF0` per packet and reuses it on each frame.
pub fn encode_air_frame_part(
    payload: &[u8],
    sequence_entropy: u8,
    index: usize,
    out: &mut [u8],
) -> Result<usize, AirFrameError> {
    if payload.len() > LORA_MAX_PAYLOAD {
        return Err(AirFrameError::PayloadExceedsMax);
    }
    let split = payload.len() > LORA_SINGLE_FRAME_PAYLOAD_MAX;
    // Clamp so an out-of-range index yields a header-only frame rather than
    // panicking; the worker only ever passes a valid index.
    let start = (index * LORA_SINGLE_FRAME_PAYLOAD_MAX).min(payload.len());
    let end = (start + LORA_SINGLE_FRAME_PAYLOAD_MAX).min(payload.len());
    let chunk = &payload[start..end];
    if out.len() < LORA_HEADER_LEN + chunk.len() {
        return Err(AirFrameError::OutputBufferTooSmall);
    }
    out[0] =
        (sequence_entropy & HEADER_SEQUENCE_NIBBLE) | if split { HEADER_FLAG_SPLIT } else { 0 };
    out[LORA_HEADER_LEN..LORA_HEADER_LEN + chunk.len()].copy_from_slice(chunk);
    Ok(LORA_HEADER_LEN + chunk.len())
}

pub fn decode_air_frame(frame: &[u8]) -> Option<AirFrame<'_>> {
    let (&header, payload) = frame.split_first()?;
    Some(AirFrame {
        sequence: header & HEADER_SEQUENCE_NIBBLE,
        is_split_fragment: header & HEADER_FLAG_SPLIT != 0,
        payload,
    })
}

/// Reassembles RNode's frames back into whole Reticulum packets. A whole-in-one
/// frame is delivered as-is; a split packet's two frames (same sequence nibble +
/// the split flag) are concatenated and delivered on the second. Mirrors the
/// firmware's receive state machine
/// ([`receive_callback`](https://github.com/markqvist/RNode_Firmware/blob/1.86/RNode_Firmware.ino#L359-L450)):
/// a non-matching sequence, or a whole frame mid-split, drops the partial.
///
/// `CAP` bounds the reassembled packet (size it to the engine MTU); bytes beyond
/// it are dropped.
pub struct LoRaReassembler<const CAP: usize> {
    in_progress_sequence: Option<u8>,
    buffer: HeaplessVec<u8, CAP>,
}

impl<const CAP: usize> LoRaReassembler<CAP> {
    pub const fn new() -> Self {
        Self {
            in_progress_sequence: None,
            buffer: HeaplessVec::new(),
        }
    }

    pub fn feed(&mut self, frame: &[u8]) -> Option<&[u8]> {
        let parsed = decode_air_frame(frame)?;
        let sequence = parsed.sequence;
        let complete;
        if parsed.is_split_fragment {
            let continuing = self.in_progress_sequence == Some(sequence);
            if !continuing {
                self.buffer.clear();
            }
            let _ = self.buffer.extend_from_slice(parsed.payload);
            if continuing {
                self.in_progress_sequence = None;
                complete = true;
            } else {
                self.in_progress_sequence = Some(sequence);
                complete = false;
            }
        } else {
            self.buffer.clear();
            let _ = self.buffer.extend_from_slice(parsed.payload);
            self.in_progress_sequence = None;
            complete = true;
        }
        if complete {
            Some(&self.buffer)
        } else {
            None
        }
    }
}

impl<const CAP: usize> Default for LoRaReassembler<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransitCapability::SameInterfaceRepeat),
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
    fn single_frame_has_no_split_flag_and_round_trips() {
        let payload = [1u8, 2, 3, 0x7E, 0xFF, 0];
        assert_eq!(air_frame_count(payload.len()), 1);
        let mut out = [0u8; 16];
        let n = encode_air_frame_part(&payload, 0xA7, 0, &mut out).unwrap();
        assert_eq!(out[0], 0xA0);
        let parsed = decode_air_frame(&out[..n]).unwrap();
        assert_eq!(parsed.sequence, 0xA0);
        assert!(!parsed.is_split_fragment);
        assert_eq!(parsed.payload, &payload);
    }

    #[test]
    fn a_payload_over_one_frame_splits_into_two_with_a_shared_split_header() {
        let payload: [u8; 300] = core::array::from_fn(|i| i as u8);
        assert_eq!(air_frame_count(payload.len()), 2);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut f1 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&payload, 0x30, 0, &mut f0).unwrap();
        let n1 = encode_air_frame_part(&payload, 0x30, 1, &mut f1).unwrap();
        assert_eq!(n0, LORA_SINGLE_FRAME_MAX);
        assert_eq!(n1, LORA_HEADER_LEN + (300 - LORA_SINGLE_FRAME_PAYLOAD_MAX));
        assert_eq!(f0[0], 0x30 | HEADER_FLAG_SPLIT);
        assert_eq!(f1[0], 0x30 | HEADER_FLAG_SPLIT);
        let p0 = decode_air_frame(&f0[..n0]).unwrap();
        let p1 = decode_air_frame(&f1[..n1]).unwrap();
        assert!(p0.is_split_fragment && p1.is_split_fragment);
        assert_eq!(p0.payload, &payload[..LORA_SINGLE_FRAME_PAYLOAD_MAX]);
        assert_eq!(p1.payload, &payload[LORA_SINGLE_FRAME_PAYLOAD_MAX..]);
    }

    #[test]
    fn reassembler_passes_a_single_frame_straight_through() {
        let mut out = [0u8; 16];
        let n = encode_air_frame_part(&[0xAA, 0xBB, 0xCC], 0x10, 0, &mut out).unwrap();
        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&out[..n]), Some(&[0xAA, 0xBB, 0xCC][..]));
    }

    #[test]
    fn reassembler_rebuilds_a_split_packet_from_its_two_frames() {
        let payload: [u8; 400] = core::array::from_fn(|i| (i * 7) as u8);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut f1 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&payload, 0x70, 0, &mut f0).unwrap();
        let n1 = encode_air_frame_part(&payload, 0x70, 1, &mut f1).unwrap();
        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&f0[..n0]), None);
        assert_eq!(r.feed(&f1[..n1]), Some(&payload[..]));
    }

    #[test]
    fn reassembler_drops_a_partial_split_when_a_whole_frame_arrives() {
        let big: [u8; 300] = core::array::from_fn(|i| i as u8);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&big, 0x20, 0, &mut f0).unwrap();
        let mut whole = [0u8; 16];
        let wn = encode_air_frame_part(&[0xEE; 4], 0x90, 0, &mut whole).unwrap();

        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&f0[..n0]), None);
        assert_eq!(r.feed(&whole[..wn]), Some(&[0xEE; 4][..]));
    }

    #[test]
    fn reassembler_restarts_on_a_new_split_sequence() {
        let a: [u8; 300] = core::array::from_fn(|i| i as u8);
        let b: [u8; 300] = core::array::from_fn(|i| (255 - i % 256) as u8);
        let mut a0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut b0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut b1 = [0u8; LORA_SINGLE_FRAME_MAX];
        let an0 = encode_air_frame_part(&a, 0x40, 0, &mut a0).unwrap();
        let bn0 = encode_air_frame_part(&b, 0x80, 0, &mut b0).unwrap();
        let bn1 = encode_air_frame_part(&b, 0x80, 1, &mut b1).unwrap();

        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&a0[..an0]), None);
        assert_eq!(r.feed(&b0[..bn0]), None);
        assert_eq!(r.feed(&b1[..bn1]), Some(&b[..]));
    }

    #[test]
    fn rejects_payload_larger_than_two_frames() {
        let payload = [0u8; LORA_MAX_PAYLOAD + 1];
        let mut out = [0u8; LORA_SINGLE_FRAME_MAX];
        assert_eq!(
            encode_air_frame_part(&payload, 0, 0, &mut out),
            Err(AirFrameError::PayloadExceedsMax)
        );
    }

    #[test]
    fn rejects_output_buffer_too_small() {
        let payload = [1u8, 2, 3];
        let mut out = [0u8; 3];
        assert_eq!(
            encode_air_frame_part(&payload, 0, 0, &mut out),
            Err(AirFrameError::OutputBufferTooSmall)
        );
    }

    #[test]
    fn decode_empty_frame_is_none() {
        assert!(decode_air_frame(&[]).is_none());
        assert_eq!(LoRaReassembler::<64>::new().feed(&[]), None);
    }

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(InterfaceId::new([0x5C; 16]));
        assert!(matches!(d.medium, MediumKind::SharedHalfDuplex));
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert!(matches!(d.state, ConnectionState::Connected));
        assert_eq!(d.capabilities.ingress, IngressCapability::Enabled);
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransitCapability::SameInterfaceRepeat)
        );
    }
}
