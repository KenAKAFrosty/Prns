pub mod byte_stream;
pub mod columns;
pub mod impls;
pub mod receive;
pub mod send;

use crate::routing::links::data::link_mdu;
use crate::units::RttMillis;

/// RNS 1.3.5 `Channel` `MSGTYPE`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MessageType(pub u16);

/// RNS 1.3.5 `Channel` sequence number: the ordering key for reliable in-order delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct ChannelSequence(pub u16);

/// RNS 1.3.5 `Channel.SEQ_MODULUS` (`SEQ_MAX + 1`)
pub const SEQUENCE_MODULUS: u32 = 0x1_0000;

impl ChannelSequence {
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ChannelRtt(pub RttMillis);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRttTier {
    Fast,
    Medium,
    Slow,
}

impl ChannelRtt {
    /// RNS 1.3.5 `Channel.RTT_FAST` (0.18 s)
    pub const FAST_CEILING: RttMillis = RttMillis::new(180);
    /// RNS 1.3.5 `Channel.RTT_MEDIUM` (0.75 s)
    pub const MEDIUM_CEILING: RttMillis = RttMillis::new(750);

    /// RNS 1.3.5 `Channel.RTT_SLOW` (1.45 s)
    pub const STOP_AND_WAIT_THRESHOLD: RttMillis = RttMillis::new(1_450);

    pub const fn tier(self) -> ChannelRttTier {
        if self.0.millis() <= Self::FAST_CEILING.millis() {
            ChannelRttTier::Fast
        } else if self.0.millis() <= Self::MEDIUM_CEILING.millis() {
            ChannelRttTier::Medium
        } else {
            ChannelRttTier::Slow
        }
    }

    pub const fn demands_stop_and_wait(self) -> bool {
        self.0.millis() > Self::STOP_AND_WAIT_THRESHOLD.millis()
    }
}

impl From<RttMillis> for ChannelRtt {
    fn from(rtt: RttMillis) -> Self {
        ChannelRtt(rtt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelWindow {
    size: u8,
    max: u8,
    min: u8,
    flexibility: u8,
    fast_rate_rounds: u32,
    medium_rate_rounds: u32,
}

impl ChannelWindow {
    pub const INITIAL: u8 = 2;
    pub const MIN: u8 = 2;
    pub const MIN_LIMIT_MEDIUM: u8 = 5;
    pub const MIN_LIMIT_FAST: u8 = 16;
    pub const MAX_SLOW: u8 = 5;
    pub const MAX_MEDIUM: u8 = 12;
    pub const MAX_FAST: u8 = 48;
    pub const FLEXIBILITY: u8 = 4;
    pub const FAST_RATE_THRESHOLD: u32 = 10;

    pub const fn for_rtt(rtt: ChannelRtt) -> Self {
        if rtt.demands_stop_and_wait() {
            Self {
                size: 1,
                max: 1,
                min: 1,
                flexibility: 1,
                fast_rate_rounds: 0,
                medium_rate_rounds: 0,
            }
        } else {
            Self {
                size: Self::INITIAL,
                max: Self::MAX_SLOW,
                min: Self::MIN,
                flexibility: Self::FLEXIBILITY,
                fast_rate_rounds: 0,
                medium_rate_rounds: 0,
            }
        }
    }

    pub const fn in_flight_count_limit(&self) -> usize {
        self.size as usize
    }

    pub fn grow_on_ack(&mut self, rtt: ChannelRtt) {
        if self.size < self.max {
            self.size += 1;
        }
        match rtt.tier() {
            ChannelRttTier::Slow => {
                self.fast_rate_rounds = 0;
                self.medium_rate_rounds = 0;
            }
            ChannelRttTier::Medium => {
                self.fast_rate_rounds = 0;
                self.medium_rate_rounds = self.medium_rate_rounds.saturating_add(1);
                if self.max < Self::MAX_MEDIUM
                    && self.medium_rate_rounds == Self::FAST_RATE_THRESHOLD
                {
                    self.max = Self::MAX_MEDIUM;
                    self.min = Self::MIN_LIMIT_MEDIUM;
                }
            }
            ChannelRttTier::Fast => {
                self.fast_rate_rounds = self.fast_rate_rounds.saturating_add(1);
                if self.max < Self::MAX_FAST && self.fast_rate_rounds == Self::FAST_RATE_THRESHOLD {
                    self.max = Self::MAX_FAST;
                    self.min = Self::MIN_LIMIT_FAST;
                }
            }
        }
    }

    pub fn shrink_on_loss(&mut self) {
        if self.size > self.min {
            self.size -= 1;
            if self.max > self.min + self.flexibility {
                self.max -= 1;
            }
        }
    }
}

impl Default for ChannelWindow {
    fn default() -> Self {
        Self::for_rtt(ChannelRtt(RttMillis::new(0)))
    }
}

pub const CHANNEL_ENVELOPE_HEADER_LEN: usize = 6;

pub const fn channel_mdu(mtu: usize) -> usize {
    let body = link_mdu(mtu).saturating_sub(CHANNEL_ENVELOPE_HEADER_LEN);
    if body > u16::MAX as usize {
        u16::MAX as usize
    } else {
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    PayloadTooLong,
    BufferTooShort,
    TruncatedHeader,
    LengthMismatch,
}

/// RNS 1.3.5 `Envelope.pack`'s header: `message_type ‖ sequence ‖ length`, each u16 BE.
pub fn write_envelope(
    message_type: MessageType,
    sequence: ChannelSequence,
    payload: &[u8],
    buf: &mut [u8],
) -> Result<usize, EnvelopeError> {
    if payload.len() > u16::MAX as usize {
        return Err(EnvelopeError::PayloadTooLong);
    }
    let end = CHANNEL_ENVELOPE_HEADER_LEN + payload.len();
    if buf.len() < end {
        return Err(EnvelopeError::BufferTooShort);
    }
    buf[0..2].copy_from_slice(&message_type.0.to_be_bytes());
    buf[2..4].copy_from_slice(&sequence.0.to_be_bytes());
    buf[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    buf[CHANNEL_ENVELOPE_HEADER_LEN..end].copy_from_slice(payload);
    Ok(end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope<'a> {
    pub message_type: MessageType,
    pub sequence: ChannelSequence,
    pub payload: &'a [u8],
}

pub fn parse_envelope(bytes: &[u8]) -> Result<Envelope<'_>, EnvelopeError> {
    if bytes.len() < CHANNEL_ENVELOPE_HEADER_LEN {
        return Err(EnvelopeError::TruncatedHeader);
    }
    let message_type = MessageType(u16::from_be_bytes([bytes[0], bytes[1]]));
    let sequence = ChannelSequence(u16::from_be_bytes([bytes[2], bytes[3]]));
    let length = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let payload = &bytes[CHANNEL_ENVELOPE_HEADER_LEN..];
    if payload.len() != length {
        return Err(EnvelopeError::LengthMismatch);
    }
    Ok(Envelope {
        message_type,
        sequence,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::BROADCAST_MTU;

    fn rtt(ms: u64) -> ChannelRtt {
        ChannelRtt(RttMillis::new(ms))
    }

    fn bytes_from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    #[test]
    fn write_envelope_matches_the_reference_pack() {
        let mut buf = [0u8; 64];
        let n = write_envelope(
            MessageType(0x0007),
            ChannelSequence(0x0003),
            b"hello channel",
            &mut buf,
        )
        .unwrap();
        assert_eq!(
            &buf[..n],
            &bytes_from_hex("00070003000d68656c6c6f206368616e6e656c")[..]
        );
    }

    #[test]
    fn the_system_stream_type_near_the_wrap_round_trips() {
        let mut buf = [0u8; 32];
        let n = write_envelope(
            MessageType(0xff00),
            ChannelSequence(0xfffe),
            &[0, 1, 2, 3],
            &mut buf,
        )
        .unwrap();
        assert_eq!(&buf[..n], &bytes_from_hex("ff00fffe000400010203")[..]);

        let envelope = parse_envelope(&buf[..n]).unwrap();
        assert_eq!(envelope.message_type, MessageType(0xff00));
        assert_eq!(envelope.sequence, ChannelSequence(0xfffe));
        assert_eq!(envelope.payload, &[0, 1, 2, 3]);
    }

    #[test]
    fn an_empty_body_packs_a_zero_length_field() {
        let mut buf = [0u8; 16];
        let n =
            write_envelope(MessageType(0x0001), ChannelSequence(0x0000), b"", &mut buf).unwrap();
        assert_eq!(&buf[..n], &bytes_from_hex("000100000000")[..]);
        assert_eq!(parse_envelope(&buf[..n]).unwrap().payload, b"");
    }

    #[test]
    fn parse_rejects_a_truncated_header() {
        assert_eq!(
            parse_envelope(&[0x00, 0x07, 0x00]),
            Err(EnvelopeError::TruncatedHeader),
        );
    }

    #[test]
    fn parse_rejects_a_length_field_that_disagrees_with_the_body() {
        assert_eq!(
            parse_envelope(&bytes_from_hex("0007000300056865")),
            Err(EnvelopeError::LengthMismatch),
        );
    }

    #[test]
    fn write_rejects_a_body_that_overflows_the_buffer() {
        let mut buf = [0u8; 8];
        assert_eq!(
            write_envelope(
                MessageType(0x0001),
                ChannelSequence(0x0000),
                b"toolong",
                &mut buf
            ),
            Err(EnvelopeError::BufferTooShort),
        );
    }

    #[test]
    fn the_sequence_wraps_past_the_modulus() {
        assert_eq!(ChannelSequence(0xFFFF).next(), ChannelSequence(0x0000));
        assert_eq!(SEQUENCE_MODULUS, 0x1_0000);
    }

    #[test]
    fn the_channel_mdu_is_the_link_mdu_less_the_header_capped_at_u16() {
        assert_eq!(channel_mdu(BROADCAST_MTU), 425);
        assert_eq!(channel_mdu(1_000_000), u16::MAX as usize);
    }

    #[test]
    fn the_window_opens_at_the_rtt_tier() {
        assert_eq!(
            ChannelWindow::for_rtt(rtt(0)).in_flight_count_limit(),
            ChannelWindow::INITIAL as usize
        );
        assert_eq!(ChannelWindow::for_rtt(rtt(100)).in_flight_count_limit(), 2);
        assert_eq!(
            ChannelWindow::for_rtt(rtt(2_000)).in_flight_count_limit(),
            1,
            "a slow link falls back to stop-and-wait"
        );
    }

    #[test]
    fn an_ack_opens_the_window_one_step_toward_its_ceiling() {
        let mut window = ChannelWindow::for_rtt(rtt(250));
        assert_eq!(window.in_flight_count_limit(), 2);
        for expected in [3, 4, 5, 5, 5] {
            window.grow_on_ack(rtt(250));
            assert_eq!(
                window.in_flight_count_limit(),
                expected,
                "grows to the ceiling then holds"
            );
        }
    }

    #[test]
    fn a_sustained_fast_run_ratchets_the_ceiling_to_the_fast_tier() {
        let mut window = ChannelWindow::for_rtt(rtt(50));
        for _ in 0..ChannelWindow::FAST_RATE_THRESHOLD {
            window.grow_on_ack(rtt(50));
        }
        for _ in 0..ChannelWindow::MAX_FAST {
            window.grow_on_ack(rtt(50));
        }
        assert_eq!(
            window.in_flight_count_limit(),
            ChannelWindow::MAX_FAST as usize
        );
    }

    #[test]
    fn a_sub_millisecond_link_earns_the_fast_tier() {
        let mut window = ChannelWindow::for_rtt(rtt(0));
        for _ in 0..ChannelWindow::FAST_RATE_THRESHOLD {
            window.grow_on_ack(rtt(0));
        }
        for _ in 0..ChannelWindow::MAX_FAST {
            window.grow_on_ack(rtt(0));
        }
        assert_eq!(
            window.in_flight_count_limit(),
            ChannelWindow::MAX_FAST as usize,
            "an rtt of zero is a measured sub-ms link, not an unmeasured one",
        );
    }

    #[test]
    fn a_sustained_medium_run_ratchets_only_to_the_medium_tier() {
        let mut window = ChannelWindow::for_rtt(rtt(500));
        let rounds = ChannelWindow::FAST_RATE_THRESHOLD + u32::from(ChannelWindow::MAX_MEDIUM);
        for _ in 0..rounds {
            window.grow_on_ack(rtt(500));
        }
        assert_eq!(
            window.in_flight_count_limit(),
            ChannelWindow::MAX_MEDIUM as usize
        );
    }

    #[test]
    fn a_loss_closes_the_window_toward_its_floor() {
        let mut window = ChannelWindow::for_rtt(rtt(250));
        for _ in 0..15 {
            window.grow_on_ack(rtt(250));
        }
        let opened = window.in_flight_count_limit();
        assert!(opened > ChannelWindow::MIN as usize);
        window.shrink_on_loss();
        assert_eq!(
            window.in_flight_count_limit(),
            opened - 1,
            "a loss closes the window by one"
        );
    }

    #[test]
    fn the_window_will_not_close_below_its_floor() {
        let mut window = ChannelWindow::for_rtt(rtt(250));
        window.shrink_on_loss();
        assert_eq!(
            window.in_flight_count_limit(),
            ChannelWindow::MIN as usize,
            "a fresh window sits at its floor and a loss cannot push it lower"
        );
    }
}
