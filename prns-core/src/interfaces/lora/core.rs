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
    InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode, TransportCapability,
};

pub const LORA_HEADER_LEN: usize = 1;

pub const LORA_SINGLE_FRAME_MAX: usize = 255;

pub const LORA_SINGLE_FRAME_PAYLOAD_MAX: usize = LORA_SINGLE_FRAME_MAX - LORA_HEADER_LEN;

pub const LORA_MAX_PAYLOAD: usize = 2 * LORA_SINGLE_FRAME_PAYLOAD_MAX;

const HEADER_SEQUENCE_NIBBLE: u8 = 0xF0;
const HEADER_FLAG_SPLIT: u8 = 0x01;

pub const RNODE_LORA_SYNC_WORD: u16 = 0x1424;

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

impl SpreadingFactor {
    pub const fn next(self) -> Self {
        match self {
            Self::Sf5 => Self::Sf6,
            Self::Sf6 => Self::Sf7,
            Self::Sf7 => Self::Sf8,
            Self::Sf8 => Self::Sf9,
            Self::Sf9 => Self::Sf10,
            Self::Sf10 => Self::Sf11,
            Self::Sf11 => Self::Sf12,
            Self::Sf12 => Self::Sf5,
        }
    }
}

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

    pub const fn next(self) -> Self {
        match self {
            Self::Bw125kHz => Self::Bw250kHz,
            Self::Bw250kHz => Self::Bw500kHz,
            Self::Bw500kHz => Self::Bw125kHz,
        }
    }
}

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

    pub const fn next(self) -> Self {
        match self {
            Self::Cr45 => Self::Cr46,
            Self::Cr46 => Self::Cr47,
            Self::Cr47 => Self::Cr48,
            Self::Cr48 => Self::Cr45,
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: LoraBandwidth,
        coding_rate: CodingRate,
    },
}

impl Modulation {
    pub const fn nominal_bitrate_bps(self) -> u32 {
        let Self::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } = self;
        let sf = spreading_factor as u32;
        let bandwidth_hz = bandwidth.hz();
        let coding_denominator = coding_rate as u32;
        (sf * bandwidth_hz * 4) / ((1u32 << sf) * coding_denominator)
    }

    /// LoRa low-data-rate optimize: on only for the slow SF/BW combos (SX126x DS 6.1.4).
    pub const fn low_data_rate_optimize(self) -> bool {
        let Self::Lora {
            spreading_factor,
            bandwidth,
            ..
        } = self;
        matches!(
            (spreading_factor, bandwidth),
            (
                SpreadingFactor::Sf11 | SpreadingFactor::Sf12,
                LoraBandwidth::Bw125kHz
            ) | (SpreadingFactor::Sf12, LoraBandwidth::Bw250kHz)
        )
    }
}

const DUTY_ONE_PERCENT_PER_MILLE: u16 = 10;
const DUTY_QUEUE_BUDGET_MS: u32 = 4_000;

const DUTY_TEN_PERCENT_PER_MILLE: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Us915,
    Au915,
    Eu433,
    Eu865,
    Eu868,
    Eu869,
    As923,
    In865,
    Cn470,
    Kr920,
    Jp920,
    Unlimited,
}

impl Region {
    pub const ALL: [Region; 12] = [
        Self::Us915,
        Self::Au915,
        Self::Eu433,
        Self::Eu865,
        Self::Eu868,
        Self::Eu869,
        Self::As923,
        Self::In865,
        Self::Cn470,
        Self::Kr920,
        Self::Jp920,
        Self::Unlimited,
    ];

    pub const fn band(self) -> (u32, u32) {
        match self {
            Self::Us915 => (902_000_000, 928_000_000),
            Self::Au915 => (915_000_000, 928_000_000),
            Self::Eu433 => (433_050_000, 434_790_000),
            Self::Eu865 => (865_000_000, 868_000_000),
            Self::Eu868 => (868_000_000, 868_600_000),
            Self::Eu869 => (869_400_000, 869_650_000),
            Self::As923 => (920_000_000, 925_000_000),
            Self::In865 => (865_000_000, 867_000_000),
            Self::Cn470 => (470_000_000, 510_000_000),
            Self::Kr920 => (920_000_000, 923_000_000),
            Self::Jp920 => (920_800_000, 927_800_000),
            Self::Unlimited => (150_000_000, 960_000_000),
        }
    }

    pub const fn default_frequency(self) -> Frequency {
        let hz = match self {
            Self::Us915 => 915_000_000,
            Self::Au915 => 921_500_000,
            Self::Eu433 => 433_900_000,
            Self::Eu865 => 866_500_000,
            Self::Eu868 => 868_300_000,
            Self::Eu869 => 869_500_000,
            Self::As923 => 922_500_000,
            Self::In865 => 866_000_000,
            Self::Cn470 => 490_000_000,
            Self::Kr920 => 921_500_000,
            Self::Jp920 => 922_000_000,
            Self::Unlimited => 915_000_000,
        };
        Frequency::new(hz)
    }

    pub const fn max_tx_power(self) -> TxPower {
        let dbm = match self {
            Self::Us915 | Self::Au915 | Self::In865 | Self::Eu869 | Self::Unlimited => 22,
            Self::Cn470 => 19,
            Self::As923 | Self::Jp920 => 16,
            Self::Eu865 | Self::Eu868 | Self::Kr920 => 14,
            Self::Eu433 => 12,
        };
        TxPower::new(dbm)
    }

    pub const fn duty_cycle(self) -> Option<AirtimeDutyCycle> {
        let limit_long_per_mille = match self {
            Self::Eu865 | Self::Eu868 => DUTY_ONE_PERCENT_PER_MILLE,
            Self::Eu433 | Self::Eu869 => DUTY_TEN_PERCENT_PER_MILLE,
            _ => return None,
        };
        Some(AirtimeDutyCycle {
            limit_short_per_mille: None,
            limit_long_per_mille: Some(limit_long_per_mille),
            max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
        })
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Us915 => "US915",
            Self::Au915 => "AU915",
            Self::Eu433 => "EU433",
            Self::Eu865 => "EU865",
            Self::Eu868 => "EU868",
            Self::Eu869 => "EU869",
            Self::As923 => "AS923",
            Self::In865 => "IN865",
            Self::Cn470 => "CN470",
            Self::Kr920 => "KR920",
            Self::Jp920 => "JP920",
            Self::Unlimited => "Custom",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Us915 => Self::Au915,
            Self::Au915 => Self::Eu433,
            Self::Eu433 => Self::Eu865,
            Self::Eu865 => Self::Eu868,
            Self::Eu868 => Self::Eu869,
            Self::Eu869 => Self::As923,
            Self::As923 => Self::In865,
            Self::In865 => Self::Cn470,
            Self::Cn470 => Self::Kr920,
            Self::Kr920 => Self::Jp920,
            Self::Jp920 => Self::Unlimited,
            Self::Unlimited => Self::Us915,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModemPreset {
    ShortFast,
    MediumFast,
    LongFast,
    LongSlow,
}

impl ModemPreset {
    pub const ALL: [ModemPreset; 4] = [
        Self::ShortFast,
        Self::MediumFast,
        Self::LongFast,
        Self::LongSlow,
    ];

    pub const fn modulation(self) -> Modulation {
        match self {
            Self::ShortFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::MediumFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf9,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::LongFast => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf11,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            },
            Self::LongSlow => Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf12,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr48,
            },
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ShortFast => "ShortFast",
            Self::MediumFast => "MediumFast",
            Self::LongFast => "LongFast",
            Self::LongSlow => "LongSlow",
        }
    }

    pub fn matching(modulation: Modulation) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.modulation() == modulation)
    }
}

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

    /// RNode firmware `add_airtime` (RNode_Firmware.ino, SX126x arm): the real on-air time of
    /// one `frame_bytes` transmission at this profile, counting what the nominal bitrate ignores
    /// (preamble, PHY header symbols, CRC bits, sync overhead, low-data-rate widening).
    /// Integer throughout; agrees with the firmware's float arithmetic to under a microsecond.
    pub const fn time_on_air_us(self, frame_bytes: usize) -> u64 {
        let Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } = self.modulation;
        let sf = spreading_factor as u128;
        let coding = coding_rate as u128;
        let bandwidth_hz = bandwidth.hz() as u128;
        let preamble = self.preamble.count() as u128;
        let bytes = frame_bytes as u128;
        let (coded_bits, quarter_denominator, tail_quarter_symbols) = if sf >= 7 {
            let ldro = if self.modulation.low_data_rate_optimize() {
                2
            } else {
                0
            };
            (
                (8 * bytes + 44).saturating_sub(4 * sf),
                sf - ldro,
                4 * preamble + 33,
            )
        } else {
            (
                (8 * bytes + 36).saturating_sub(4 * sf),
                sf,
                4 * preamble + 41,
            )
        };
        let payload_us =
            coded_bits * coding * (1 << sf) * 1_000_000 / (4 * quarter_denominator * bandwidth_hz);
        let tail_us = tail_quarter_symbols * (1 << sf) * 250_000 / bandwidth_hz;
        (payload_us + tail_us) as u64
    }
}

pub const DEFAULT_915_PROFILE: RadioProfile = RadioProfile {
    frequency: Frequency::new(915_000_000),
    modulation: ModemPreset::LongFast.modulation(),
    tx_power: TxPower::new(22),
    preamble: PreambleSymbols::new(18),
    region: Region::Us915,
};

const MODULATION_TAG_LORA: u8 = 0x00;

pub const CHANNEL_TAG_CAP: usize = 11;

pub fn channel_tag(profile: &RadioProfile) -> HeaplessVec<u8, CHANNEL_TAG_CAP> {
    let mut tag = HeaplessVec::new();
    let _ = tag.extend_from_slice(&profile.frequency.hz().to_be_bytes());
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let _ = tag.push(MODULATION_TAG_LORA);
    let _ = tag.push(spreading_factor as u8);
    let _ = tag.extend_from_slice(&bandwidth.hz().to_be_bytes());
    let _ = tag.push(coding_rate as u8);
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

pub fn descriptor(id: InterfaceId, profile: &RadioProfile) -> InterfaceDescriptor {
    InterfaceDescriptor {
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
    fn time_on_air_matches_the_rnode_firmware_formula() {
        assert_eq!(
            DEFAULT_915_PROFILE.time_on_air_us(167),
            1_458_734,
            "a 167-byte announce on LongFast (SF11/250k/CR4:5, 18-symbol preamble)"
        );
        let long_slow = RadioProfile {
            modulation: ModemPreset::LongSlow.modulation(),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(
            long_slow.time_on_air_us(255),
            14_203_289,
            "a full frame on LongSlow (SF12/125k/CR4:8, low-data-rate optimize on)"
        );
        let sub_sf7 = RadioProfile {
            modulation: Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf6,
                bandwidth: LoraBandwidth::Bw500kHz,
                coding_rate: CodingRate::Cr45,
            },
            preamble: PreambleSymbols::new(12),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(
            sub_sf7.time_on_air_us(50),
            13_834,
            "the sub-SF7 symbol layout uses its own header and tail shape"
        );
    }

    #[test]
    fn time_on_air_exceeds_the_nominal_serialization_time() {
        let nominal_us =
            167u64 * 8 * 1_000_000 / u64::from(DEFAULT_915_PROFILE.nominal_bitrate_bps());
        assert!(DEFAULT_915_PROFILE.time_on_air_us(167) > nominal_us);
    }

    #[test]
    fn low_data_rate_optimize_covers_exactly_the_slow_combos() {
        let slow_combos = [
            (SpreadingFactor::Sf11, LoraBandwidth::Bw125kHz),
            (SpreadingFactor::Sf12, LoraBandwidth::Bw125kHz),
            (SpreadingFactor::Sf12, LoraBandwidth::Bw250kHz),
        ];
        for sf in [
            SpreadingFactor::Sf5,
            SpreadingFactor::Sf6,
            SpreadingFactor::Sf7,
            SpreadingFactor::Sf8,
            SpreadingFactor::Sf9,
            SpreadingFactor::Sf10,
            SpreadingFactor::Sf11,
            SpreadingFactor::Sf12,
        ] {
            for bandwidth in [
                LoraBandwidth::Bw125kHz,
                LoraBandwidth::Bw250kHz,
                LoraBandwidth::Bw500kHz,
            ] {
                let modulation = Modulation::Lora {
                    spreading_factor: sf,
                    bandwidth,
                    coding_rate: CodingRate::Cr45,
                };
                assert_eq!(
                    modulation.low_data_rate_optimize(),
                    slow_combos.contains(&(sf, bandwidth)),
                );
            }
        }
    }

    #[test]
    fn each_setting_cycles_through_all_its_values_and_returns_to_start() {
        let mut sf = SpreadingFactor::Sf5;
        for _ in 0..8 {
            sf = sf.next();
        }
        assert_eq!(sf, SpreadingFactor::Sf5);
        assert_eq!(SpreadingFactor::Sf12.next(), SpreadingFactor::Sf5);

        let mut bw = LoraBandwidth::Bw125kHz;
        for _ in 0..3 {
            bw = bw.next();
        }
        assert_eq!(bw, LoraBandwidth::Bw125kHz);

        let mut cr = CodingRate::Cr45;
        for _ in 0..4 {
            cr = cr.next();
        }
        assert_eq!(cr, CodingRate::Cr45);

        let mut region = Region::Us915;
        for _ in 0..Region::ALL.len() {
            region = region.next();
        }
        assert_eq!(region, Region::Us915);
    }

    #[test]
    fn every_region_default_frequency_sits_inside_its_band_and_within_the_radio_pa() {
        for region in Region::ALL {
            let (lo, hi) = region.band();
            let default = region.default_frequency().hz();
            assert!(
                (lo..=hi).contains(&default),
                "{}: default {default} outside band {lo}..={hi}",
                region.label()
            );
            assert!(
                region.max_tx_power().dbm() <= 22,
                "{}: power cap above the SX1262 PA",
                region.label()
            );
        }
    }

    #[test]
    fn modem_presets_round_trip_through_their_modulation() {
        for preset in ModemPreset::ALL {
            assert_eq!(ModemPreset::matching(preset.modulation()), Some(preset));
        }
        assert_eq!(ModemPreset::LongFast.modulation().nominal_bitrate_bps(), {
            Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf11,
                bandwidth: LoraBandwidth::Bw250kHz,
                coding_rate: CodingRate::Cr45,
            }
            .nominal_bitrate_bps()
        });
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
    fn region_duty_cycles_follow_the_eu_subband_rules() {
        let eu868 = Region::Eu868.duty_cycle().expect("EU 868 is duty-limited");
        assert_eq!(
            eu868.limit_long_per_mille,
            Some(10),
            "1% over the hour window"
        );
        assert_eq!(eu868.limit_short_per_mille, None);
        assert_eq!(
            Region::Eu433
                .duty_cycle()
                .expect("EU 433 is duty-limited")
                .limit_long_per_mille,
            Some(100),
            "10% over the hour window"
        );
        assert_eq!(
            Region::Eu869.duty_cycle().unwrap().limit_long_per_mille,
            Some(100)
        );
        assert!(
            Region::Us915.duty_cycle().is_none(),
            "the US band is dwell-time, not duty-cycled"
        );
        assert!(Region::As923.duty_cycle().is_none());
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
