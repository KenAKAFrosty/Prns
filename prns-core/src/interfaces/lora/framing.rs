use heapless::Vec as HeaplessVec;

use crate::interfaces::{PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb};

pub const LORA_HEADER_LEN: usize = 1;
pub const LORA_SINGLE_FRAME_MAX: usize = 255;
pub const LORA_SINGLE_FRAME_PAYLOAD_MAX: usize = LORA_SINGLE_FRAME_MAX - LORA_HEADER_LEN;
pub const LORA_MAX_PAYLOAD: usize = 2 * LORA_SINGLE_FRAME_PAYLOAD_MAX;

const HEADER_SEQUENCE_NIBBLE: u8 = 0xF0;
const HEADER_FLAG_SPLIT: u8 = 0x01;

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
    state: ReassemblyState,
    buffer: HeaplessVec<u8, CAP>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReassemblyState {
    Idle,
    AwaitingSecond { sequence: u8, phy: PacketPhyStats },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReassembledPacket<'a> {
    pub bytes: &'a [u8],
    pub phy: PacketPhyStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaReassemblyError {
    EmptyAirFrame,
    CapacityExceeded,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoRaReassemblyOutcome<'a> {
    AwaitingSecond,
    Delivered(ReassembledPacket<'a>),
    ReplacedPartialAndAwaitingSecond,
    DeliveredAfterReplacingPartial(ReassembledPacket<'a>),
    Rejected(LoRaReassemblyError),
}

impl<const CAP: usize> LoRaReassembler<CAP> {
    pub const fn new() -> Self {
        Self {
            state: ReassemblyState::Idle,
            buffer: HeaplessVec::new(),
        }
    }

    pub fn feed(&mut self, frame: &[u8]) -> LoRaReassemblyOutcome<'_> {
        self.feed_with_phy(frame, PacketPhyStats::default())
    }

    pub fn feed_with_phy(
        &mut self,
        frame: &[u8],
        phy: PacketPhyStats,
    ) -> LoRaReassemblyOutcome<'_> {
        let Some(parsed) = decode_air_frame(frame) else {
            return LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::EmptyAirFrame);
        };
        if !parsed.is_split_fragment {
            let replaced_partial = matches!(self.state, ReassemblyState::AwaitingSecond { .. });
            self.buffer.clear();
            if self.buffer.extend_from_slice(parsed.payload).is_err() {
                self.state = ReassemblyState::Idle;
                return LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::CapacityExceeded);
            }
            self.state = ReassemblyState::Idle;
            let packet = ReassembledPacket {
                bytes: &self.buffer,
                phy,
            };
            return if replaced_partial {
                LoRaReassemblyOutcome::DeliveredAfterReplacingPartial(packet)
            } else {
                LoRaReassemblyOutcome::Delivered(packet)
            };
        }

        let ReassemblyState::AwaitingSecond {
            sequence,
            phy: first_phy,
        } = self.state
        else {
            return match self.begin_split(parsed.sequence, parsed.payload, phy) {
                Ok(()) => LoRaReassemblyOutcome::AwaitingSecond,
                Err(error) => LoRaReassemblyOutcome::Rejected(error),
            };
        };
        if sequence != parsed.sequence {
            return match self.begin_split(parsed.sequence, parsed.payload, phy) {
                Ok(()) => LoRaReassemblyOutcome::ReplacedPartialAndAwaitingSecond,
                Err(error) => LoRaReassemblyOutcome::Rejected(error),
            };
        }
        if self.buffer.extend_from_slice(parsed.payload).is_err() {
            self.buffer.clear();
            self.state = ReassemblyState::Idle;
            return LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::CapacityExceeded);
        }
        self.state = ReassemblyState::Idle;
        LoRaReassemblyOutcome::Delivered(ReassembledPacket {
            bytes: &self.buffer,
            phy: average_phy(first_phy, phy),
        })
    }

    fn begin_split(
        &mut self,
        sequence: u8,
        payload: &[u8],
        phy: PacketPhyStats,
    ) -> Result<(), LoRaReassemblyError> {
        self.buffer.clear();
        if self.buffer.extend_from_slice(payload).is_err() {
            self.state = ReassemblyState::Idle;
            return Err(LoRaReassemblyError::CapacityExceeded);
        }
        self.state = ReassemblyState::AwaitingSecond { sequence, phy };
        Ok(())
    }
}

fn average_phy(first: PacketPhyStats, second: PacketPhyStats) -> PacketPhyStats {
    let rssi = average_measurement(
        first.rssi.map(|value| i32::from(value.get())),
        second.rssi.map(|value| i32::from(value.get())),
    )
    .map(|value| RssiDbm::new(value as i16));
    let snr = average_measurement(
        first.snr.map(|value| i32::from(value.quarters())),
        second.snr.map(|value| i32::from(value.quarters())),
    )
    .map(|value| SnrQuarterDb::new(value as i16));
    let quality = average_measurement(
        first.quality.map(|value| i32::from(value.tenths_percent())),
        second
            .quality
            .map(|value| i32::from(value.tenths_percent())),
    )
    .and_then(|value| SignalQualityTenthsPercent::new(value as u16));
    PacketPhyStats { rssi, snr, quality }
}

fn average_measurement(first: Option<i32>, second: Option<i32>) -> Option<i32> {
    match (first, second) {
        (Some(first), Some(second)) => Some((first + second) / 2),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

impl<const CAP: usize> Default for LoRaReassembler<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(
            r.feed(&out[..n]),
            LoRaReassemblyOutcome::Delivered(ReassembledPacket {
                bytes: &[0xAA, 0xBB, 0xCC],
                phy: PacketPhyStats::default(),
            })
        );
    }

    #[test]
    fn reassembler_rebuilds_a_split_packet_from_its_two_frames() {
        let payload: [u8; 400] = core::array::from_fn(|i| (i * 7) as u8);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut f1 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&payload, 0x70, 0, &mut f0).unwrap();
        let n1 = encode_air_frame_part(&payload, 0x70, 1, &mut f1).unwrap();
        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&f0[..n0]), LoRaReassemblyOutcome::AwaitingSecond);
        assert_eq!(
            r.feed(&f1[..n1]),
            LoRaReassemblyOutcome::Delivered(ReassembledPacket {
                bytes: &payload,
                phy: PacketPhyStats::default(),
            })
        );
    }

    #[test]
    fn reassembler_averages_available_split_frame_phy_measurements() {
        let payload: [u8; 400] = core::array::from_fn(|i| (i * 7) as u8);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut f1 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&payload, 0x70, 0, &mut f0).unwrap();
        let n1 = encode_air_frame_part(&payload, 0x70, 1, &mut f1).unwrap();
        let mut r = LoRaReassembler::<512>::new();
        let first_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-101)),
            snr: Some(SnrQuarterDb::new(-9)),
            quality: SignalQualityTenthsPercent::new(400),
        };
        let second_phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(-80)),
            snr: Some(SnrQuarterDb::new(5)),
            quality: None,
        };
        assert_eq!(
            r.feed_with_phy(&f0[..n0], first_phy),
            LoRaReassemblyOutcome::AwaitingSecond
        );
        assert_eq!(
            r.feed_with_phy(&f1[..n1], second_phy),
            LoRaReassemblyOutcome::Delivered(ReassembledPacket {
                bytes: &payload,
                phy: PacketPhyStats {
                    rssi: Some(RssiDbm::new(-90)),
                    snr: Some(SnrQuarterDb::new(-2)),
                    quality: SignalQualityTenthsPercent::new(400),
                },
            })
        );
    }

    #[test]
    fn reassembler_drops_a_partial_split_when_a_whole_frame_arrives() {
        let big: [u8; 300] = core::array::from_fn(|i| i as u8);
        let mut f0 = [0u8; LORA_SINGLE_FRAME_MAX];
        let n0 = encode_air_frame_part(&big, 0x20, 0, &mut f0).unwrap();
        let mut whole = [0u8; 16];
        let wn = encode_air_frame_part(&[0xEE; 4], 0x90, 0, &mut whole).unwrap();

        let mut r = LoRaReassembler::<512>::new();
        assert_eq!(r.feed(&f0[..n0]), LoRaReassemblyOutcome::AwaitingSecond);
        assert_eq!(
            r.feed(&whole[..wn]),
            LoRaReassemblyOutcome::DeliveredAfterReplacingPartial(ReassembledPacket {
                bytes: &[0xEE; 4],
                phy: PacketPhyStats::default(),
            })
        );
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
        assert_eq!(r.feed(&a0[..an0]), LoRaReassemblyOutcome::AwaitingSecond);
        assert_eq!(
            r.feed(&b0[..bn0]),
            LoRaReassemblyOutcome::ReplacedPartialAndAwaitingSecond
        );
        assert_eq!(
            r.feed(&b1[..bn1]),
            LoRaReassemblyOutcome::Delivered(ReassembledPacket {
                bytes: &b,
                phy: PacketPhyStats::default(),
            })
        );
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
    fn decode_empty_frame_is_rejected() {
        assert!(decode_air_frame(&[]).is_none());
        assert_eq!(
            LoRaReassembler::<64>::new().feed(&[]),
            LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::EmptyAirFrame)
        );
    }

    #[test]
    fn reassembler_rejects_a_frame_that_exceeds_its_capacity() {
        let mut out = [0u8; 16];
        let n = encode_air_frame_part(&[0xAA; 8], 0x10, 0, &mut out).unwrap();

        assert_eq!(
            LoRaReassembler::<4>::new().feed(&out[..n]),
            LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::CapacityExceeded)
        );
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

                let outcome = reassembler.feed(&out[..n]);
                if index + 1 == frame_count {
                    prop_assert_eq!(
                        outcome,
                        LoRaReassemblyOutcome::Delivered(ReassembledPacket {
                            bytes: payload.as_slice(),
                            phy: PacketPhyStats::default(),
                        })
                    );
                } else {
                    prop_assert_eq!(outcome, LoRaReassemblyOutcome::AwaitingSecond);
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
