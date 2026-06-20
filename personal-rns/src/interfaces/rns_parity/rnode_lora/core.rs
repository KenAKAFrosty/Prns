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
//! This module mints, splits, parses, and reassembles those frames; the per-HAL
//! worker owns the SX1262 and the modulation.

use heapless::Vec as HeaplessVec;

use crate::interfaces::{
    AirtimeDutyCycle, AnnounceBandwidthCap, EgressCapability, IngressCapability,
    InterfaceCapabilities, InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
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

/// LoRa spreading factor — the chips-per-symbol exponent. The SX1262 supports
/// SF5–SF12 (RNode/Reticulum stay in SF7–SF12 in practice); higher is longer
/// range and slower. The discriminant is the factor itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpreadingFactor {
    Sf5 = 5,
    Sf6 = 6,
    Sf7 = 7,
    Sf8 = 8,
    Sf9 = 9,
    Sf10 = 10,
    Sf11 = 11,
    Sf12 = 12,
}

/// LoRa signal bandwidth. The three values Reticulum and RNode use in practice;
/// the SX1262's narrow bandwidths (down to 7.81 kHz) are a mechanical addition
/// when a profile needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraBandwidth {
    Bw125kHz,
    Bw250kHz,
    Bw500kHz,
}

impl LoraBandwidth {
    pub const fn hz(self) -> u32 {
        match self {
            Self::Bw125kHz => 125_000,
            Self::Bw250kHz => 250_000,
            Self::Bw500kHz => 500_000,
        }
    }
}

/// LoRa forward-error-correction coding rate `4/(4+n)`. The discriminant is the
/// denominator (`4/5`..`4/8`), the value the SX1262 and the bitrate formula want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodingRate {
    Cr45 = 5,
    Cr46 = 6,
    Cr47 = 7,
    Cr48 = 8,
}

impl CodingRate {
    pub const fn denominator(self) -> u8 {
        self as u8
    }
}

/// SX1262 GFSK pulse shaping — the Gaussian filter's BT product, or none. A
/// discrete chip option, so an enum, not a float. GMSK is the `GaussianBt05` case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PulseShaping {
    Off = 0,
    GaussianBt03 = 1,
    GaussianBt05 = 2,
    GaussianBt07 = 3,
    GaussianBt10 = 4,
}

/// GFSK on-air bitrate, bits per second. The SX1262 spans 0.6–300 kbps; the
/// "speed mode" we build toward targets 300k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfskBitrate(u32);

impl GfskBitrate {
    pub const fn new(bps: u32) -> Self {
        Self(bps)
    }

    pub const fn bps(self) -> u32 {
        self.0
    }
}

/// FSK frequency deviation, Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deviation(u32);

impl Deviation {
    pub const fn new(hz: u32) -> Self {
        Self(hz)
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

/// Carrier frequency, Hz. The SX1262 PLL covers roughly 150–960 MHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frequency(u32);

impl Frequency {
    pub const fn new(hz: u32) -> Self {
        Self(hz)
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

/// Transmit power, dBm. The SX1262 PA covers -9..=22 dBm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxPower(i8);

impl TxPower {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Preamble length in symbols. RNode's firmware default is 18.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreambleSymbols(u16);

impl PreambleSymbols {
    pub const fn new(count: u16) -> Self {
        Self(count)
    }

    pub const fn count(self) -> u16 {
        self.0
    }
}

/// The on-air modulation, a setting on the [`RadioProfile`]. Flipping between
/// LoRa and GFSK is the same kind of change as nudging a spreading factor — both
/// re-key the channel's identity through the channel tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: LoraBandwidth,
        coding_rate: CodingRate,
    },
    Gfsk {
        bitrate: GfskBitrate,
        deviation: Deviation,
        shaping: PulseShaping,
    },
}

impl Modulation {
    /// Nominal on-air bitrate, bits per second. For LoRa, the standard rate
    /// `SF * BW * 4 / (2^SF * (4+CR))` evaluated in integer arithmetic; for GFSK,
    /// the configured rate verbatim. Feeds the descriptor's bitrate and, with it,
    /// the airtime accounting.
    pub const fn nominal_bitrate_bps(self) -> u32 {
        match self {
            Self::Lora {
                spreading_factor,
                bandwidth,
                coding_rate,
            } => {
                let sf = spreading_factor as u32;
                let bandwidth_hz = bandwidth.hz();
                let coding_denominator = coding_rate as u32;
                (sf * bandwidth_hz * 4) / ((1u32 << sf) * coding_denominator)
            }
            Self::Gfsk { bitrate, .. } => bitrate.bps(),
        }
    }
}

/// One percent, in the per-mille the airtime ledger speaks.
const DUTY_ONE_PERCENT_PER_MILLE: u16 = 10;
/// How much projected transmit airtime a duty-limited interface queues before it drops the oldest
/// frame — the gate's budget on a slow shared medium.
const DUTY_QUEUE_BUDGET_MS: u32 = 4_000;

/// The radio regulatory region — what bounds an interface's transmit airtime. Sub-GHz ISM bands
/// carry different rules: the EU's 868 MHz band caps airtime at a 1% duty cycle, while the US
/// 902-928 and Australian bands are dwell-time limited rather than duty-cycled, so they declare no
/// ceiling here. A starting set — tune per deployment. The region drives only local TX pacing; it
/// is outside the [`channel_tag`], so two nodes on different duty policies still hear each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// EU 863-870 MHz: a 1% duty cycle over the hour window.
    Eu868,
    /// US 902-928 MHz: dwell-time limited, no duty cycle.
    Us915,
    /// Australia 915-928 MHz: dwell-time limited, no duty cycle.
    Au915,
    /// Asia 920-925 MHz: a conservative 1% (the rule varies by country).
    As923,
    /// No airtime ceiling — bench, testing, or an unregulated deployment.
    Unlimited,
}

impl Region {
    /// The airtime ceiling this region declares — host-enforced by the interface's duty gate — or
    /// `None` for a region with no duty cycle. The 1% bands cap the hour-window utilization; the
    /// queue budget bounds the airtime an over-limit interface holds before dropping the oldest.
    pub const fn duty_cycle(self) -> Option<AirtimeDutyCycle> {
        match self {
            Self::Eu868 | Self::As923 => Some(AirtimeDutyCycle {
                limit_short_per_mille: None,
                limit_long_per_mille: Some(DUTY_ONE_PERCENT_PER_MILLE),
                max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
            }),
            Self::Us915 | Self::Au915 | Self::Unlimited => None,
        }
    }
}

/// The full radio configuration both endpoints must agree on for a channel. The
/// channel tag hashes over [`frequency`](Self::frequency) and
/// [`modulation`](Self::modulation) — the settings that decide *who can hear whom*
/// — so a change to either re-keys the interface's identity. Transmit power,
/// preamble, and region are local knobs, deliberately outside the tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioProfile {
    pub frequency: Frequency,
    pub modulation: Modulation,
    pub tx_power: TxPower,
    pub preamble: PreambleSymbols,
    pub region: Region,
}

impl RadioProfile {
    pub const fn nominal_bitrate_bps(self) -> u32 {
        self.modulation.nominal_bitrate_bps()
    }
}

/// A sensible US-band LoRa starting point: 915 MHz (dwell-time limited, so no duty cycle),
/// SF8 / 125 kHz / 4-5. The preamble is RNode's firmware default (18 symbols); transmit power is a
/// placeholder pending the regional power tables.
pub const DEFAULT_915_PROFILE: RadioProfile = RadioProfile {
    frequency: Frequency::new(915_000_000),
    modulation: Modulation::Lora {
        spreading_factor: SpreadingFactor::Sf8,
        bandwidth: LoraBandwidth::Bw125kHz,
        coding_rate: CodingRate::Cr45,
    },
    tx_power: TxPower::new(22),
    preamble: PreambleSymbols::new(18),
    region: Region::Us915,
};

const MODULATION_TAG_LORA: u8 = 0x00;
const MODULATION_TAG_GFSK: u8 = 0x01;

/// The widest channel tag [`channel_tag`] emits: frequency (4) plus the
/// GFSK block (kind 1 + bitrate 4 + deviation 4 + shaping 1).
pub const CHANNEL_TAG_CAP: usize = 14;

/// The channel-identity bytes the interface id hashes over — a canonical encoding
/// of the profile's frequency and modulation. Two nodes on the same channel derive
/// the same tag (so they share an interface id); any settings change yields a new
/// one (so it re-keys). Transmit power, preamble, and region are excluded as local-only.
pub fn channel_tag(profile: &RadioProfile) -> HeaplessVec<u8, CHANNEL_TAG_CAP> {
    let mut tag = HeaplessVec::new();
    let _ = tag.extend_from_slice(&profile.frequency.hz().to_be_bytes());
    match profile.modulation {
        Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } => {
            let _ = tag.push(MODULATION_TAG_LORA);
            let _ = tag.push(spreading_factor as u8);
            let _ = tag.extend_from_slice(&bandwidth.hz().to_be_bytes());
            let _ = tag.push(coding_rate as u8);
        }
        Modulation::Gfsk {
            bitrate,
            deviation,
            shaping,
        } => {
            let _ = tag.push(MODULATION_TAG_GFSK);
            let _ = tag.extend_from_slice(&bitrate.bps().to_be_bytes());
            let _ = tag.extend_from_slice(&deviation.hz().to_be_bytes());
            let _ = tag.push(shaping as u8);
        }
    }
    tag
}

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

/// The repeating, shared-medium descriptor a LoRa interface declares. The bitrate
/// is derived from the profile's modulation, never passed independently, so it can
/// never disagree with the radio. The airtime duty cycle is left unset here — the
/// gate that enforces it is wired in its own step.
pub fn descriptor(id: InterfaceId, profile: &RadioProfile) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: Some(profile.nominal_bitrate_bps()),
        hardware_mtu: Some(LORA_MAX_PAYLOAD),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: profile.region.duty_cycle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{InterfaceKind, INTERFACE_ID_LEN};
    use proptest::prelude::*;

    fn payloads() -> impl Strategy<Value = std::vec::Vec<u8>> {
        prop::collection::vec(any::<u8>(), 0..=LORA_MAX_PAYLOAD)
    }

    fn chunk_bounds(payload_len: usize, index: usize) -> (usize, usize) {
        let start = (index * LORA_SINGLE_FRAME_PAYLOAD_MAX).min(payload_len);
        let end = (start + LORA_SINGLE_FRAME_PAYLOAD_MAX).min(payload_len);
        (start, end)
    }

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
    fn lora_nominal_bitrate_matches_the_standard_formula() {
        let slow = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf8,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        assert_eq!(slow.nominal_bitrate_bps(), 3125);
        let fast = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf7,
            bandwidth: LoraBandwidth::Bw500kHz,
            coding_rate: CodingRate::Cr45,
        };
        assert_eq!(fast.nominal_bitrate_bps(), 21875);
    }

    #[test]
    fn gfsk_nominal_bitrate_is_its_configured_rate() {
        let speed = Modulation::Gfsk {
            bitrate: GfskBitrate::new(300_000),
            deviation: Deviation::new(75_000),
            shaping: PulseShaping::GaussianBt05,
        };
        assert_eq!(speed.nominal_bitrate_bps(), 300_000);
    }

    #[test]
    fn changing_the_channel_settings_re_keys_the_interface_id() {
        let a = DEFAULT_915_PROFILE;
        let mut b = DEFAULT_915_PROFILE;
        b.modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf10,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        let id_a = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        let id_b = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&b));
        assert_ne!(id_a, id_b);
        let id_a_again = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&a));
        assert_eq!(id_a, id_a_again);
    }

    #[test]
    fn local_knobs_do_not_re_key_identity() {
        let mut low = DEFAULT_915_PROFILE;
        let mut high = DEFAULT_915_PROFILE;
        low.tx_power = TxPower::new(2);
        high.tx_power = TxPower::new(22);
        high.preamble = PreambleSymbols::new(24);
        assert_eq!(channel_tag(&low), channel_tag(&high));
    }

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(
            InterfaceId::new([0x5C; INTERFACE_ID_LEN]),
            &DEFAULT_915_PROFILE,
        );
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert_eq!(d.capabilities.ingress, IngressCapability::Enabled);
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
        assert_eq!(d.hardware_mtu, Some(LORA_MAX_PAYLOAD));
        assert_eq!(
            d.bitrate_bps,
            Some(DEFAULT_915_PROFILE.nominal_bitrate_bps())
        );
        assert_eq!(d.announce_bandwidth_cap, AnnounceBandwidthCap::RNS_DEFAULT);
    }

    #[test]
    fn the_one_percent_regions_cap_the_hour_and_the_americas_declare_no_duty() {
        let eu = Region::Eu868
            .duty_cycle()
            .expect("the EU band is duty-limited");
        assert_eq!(eu.limit_long_per_mille, Some(10), "1% over the hour window");
        assert_eq!(eu.limit_short_per_mille, None);
        assert_eq!(Region::As923.duty_cycle(), Region::Eu868.duty_cycle());
        assert!(
            Region::Us915.duty_cycle().is_none(),
            "the US band is dwell-time, not duty-cycled"
        );
        assert!(Region::Au915.duty_cycle().is_none());
        assert!(Region::Unlimited.duty_cycle().is_none());
    }

    #[test]
    fn the_descriptor_carries_the_region_duty_cycle() {
        let mut eu = DEFAULT_915_PROFILE;
        eu.region = Region::Eu868;
        let d = descriptor(InterfaceId::new([0x5C; INTERFACE_ID_LEN]), &eu);
        assert_eq!(d.airtime_duty_cycle, Region::Eu868.duty_cycle());
        let us = descriptor(
            InterfaceId::new([0x5C; INTERFACE_ID_LEN]),
            &DEFAULT_915_PROFILE,
        );
        assert_eq!(
            us.airtime_duty_cycle, None,
            "the US default declares no duty cycle"
        );
    }

    #[test]
    fn region_is_a_local_knob_outside_the_channel_tag() {
        let mut a = DEFAULT_915_PROFILE;
        let mut b = DEFAULT_915_PROFILE;
        a.region = Region::Eu868;
        b.region = Region::Unlimited;
        assert_eq!(channel_tag(&a), channel_tag(&b));
    }

    proptest! {
        #[test]
        fn arbitrary_payloads_round_trip_through_air_frames_and_reassembly(
            payload in payloads(),
            sequence_entropy in any::<u8>(),
        ) {
            let frame_count = air_frame_count(payload.len());
            let split = payload.len() > LORA_SINGLE_FRAME_PAYLOAD_MAX;
            let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();

            for index in 0..frame_count {
                let mut out = [0u8; LORA_SINGLE_FRAME_MAX];
                let n = encode_air_frame_part(&payload, sequence_entropy, index, &mut out).unwrap();
                let parsed = decode_air_frame(&out[..n]).unwrap();
                let (start, end) = chunk_bounds(payload.len(), index);

                prop_assert_eq!(parsed.sequence, sequence_entropy & HEADER_SEQUENCE_NIBBLE);
                prop_assert_eq!(parsed.is_split_fragment, split);
                prop_assert_eq!(parsed.payload, &payload[start..end]);

                let delivered = reassembler.feed(&out[..n]);
                if index + 1 == frame_count {
                    prop_assert_eq!(delivered, Some(payload.as_slice()));
                } else {
                    prop_assert_eq!(delivered, None);
                }
            }
        }

        #[test]
        fn valid_parts_fit_exact_buffers_and_reject_one_byte_shorter_buffers(
            payload in payloads(),
            sequence_entropy in any::<u8>(),
        ) {
            for index in 0..air_frame_count(payload.len()) {
                let (start, end) = chunk_bounds(payload.len(), index);
                let exact_len = LORA_HEADER_LEN + (end - start);

                let mut exact = std::vec![0u8; exact_len];
                let written =
                    encode_air_frame_part(&payload, sequence_entropy, index, &mut exact).unwrap();
                prop_assert_eq!(written, exact_len);

                let mut short = std::vec![0u8; exact_len.saturating_sub(1)];
                prop_assert_eq!(
                    encode_air_frame_part(&payload, sequence_entropy, index, &mut short),
                    Err(AirFrameError::OutputBufferTooSmall)
                );
            }
        }

        #[test]
        fn out_of_range_indices_emit_header_only_frames_without_panicking(
            payload in payloads(),
            sequence_entropy in any::<u8>(),
            extra_index in 0usize..8,
        ) {
            let index = air_frame_count(payload.len()) + extra_index;
            let mut out = [0u8; LORA_HEADER_LEN];
            let n = encode_air_frame_part(&payload, sequence_entropy, index, &mut out).unwrap();
            let parsed = decode_air_frame(&out[..n]).unwrap();

            prop_assert_eq!(n, LORA_HEADER_LEN);
            prop_assert_eq!(parsed.sequence, sequence_entropy & HEADER_SEQUENCE_NIBBLE);
            prop_assert_eq!(
                parsed.is_split_fragment,
                payload.len() > LORA_SINGLE_FRAME_PAYLOAD_MAX
            );
            prop_assert!(parsed.payload.is_empty());
        }
    }
}
