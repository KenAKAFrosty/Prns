//! Platform-agnostic part of the ESP-NOW interface: its routing descriptor and
//! the on-air frame codec.
//!
//! ESP-NOW is a Personal-native broadcast medium with no stock-RNS counterpart —
//! a Hopspot-to-Hopspot link over the 2.4 GHz radio. Both ends are ours, so the
//! frame format is ours. A v2 ESP-NOW frame carries up to
//! [`ESP_NOW_MAX_FRAME_PAYLOAD`] bytes (1470) — far above Reticulum's 500-byte
//! [`MTU`](crate::wire::MTU) — so a whole packet always fits one frame and we
//! never fragment.
//!
//! Instead we spend that headroom the other way: **coalescing**. Because several
//! Reticulum packets fit one frame, the worker packs as many as are queued (on a
//! short timer) into one frame and transmits once, amortizing the fixed
//! per-transmission radio overhead. A frame is a one-byte version tag followed by
//! length-delimited packets: [`EspNowFrameWriter`] packs them on the way out,
//! [`decode_frame`] walks them back out on the way in. Reassembly is unnecessary —
//! un-coalescing one received frame yields N whole packets, each handed to the
//! engine as-is.

use crate::interfaces::{
    ConnectionState, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind, TransportCapability,
};

pub const ESP_NOW_FRAME_VERSION: u8 = 1;

/// The largest payload one ESP-NOW frame carries: ESP-NOW v2's
/// `ESP_NOW_MAX_DATA_LEN_V2` (1470 bytes). Well above Reticulum's 500-byte MTU,
/// which is what makes coalescing — rather than fragmentation — the design.
pub const ESP_NOW_MAX_FRAME_PAYLOAD: usize = 1470;

pub const ESP_NOW_FRAME_HEADER_LEN: usize = 1;

pub const ESP_NOW_LENGTH_PREFIX_LEN: usize = 2;

pub struct EspNowFrameWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
    packet_count: usize,
}

impl<'a> EspNowFrameWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        buf[0] = ESP_NOW_FRAME_VERSION;
        Self {
            buf,
            len: ESP_NOW_FRAME_HEADER_LEN,
            packet_count: 0,
        }
    }

    pub fn try_push(&mut self, packet: &[u8]) -> bool {
        if packet.len() > u16::MAX as usize {
            return false;
        }
        let need = ESP_NOW_LENGTH_PREFIX_LEN + packet.len();
        if self.len + need > self.buf.len() {
            return false;
        }
        self.buf[self.len..self.len + ESP_NOW_LENGTH_PREFIX_LEN]
            .copy_from_slice(&(packet.len() as u16).to_be_bytes());
        self.len += ESP_NOW_LENGTH_PREFIX_LEN;
        self.buf[self.len..self.len + packet.len()].copy_from_slice(packet);
        self.len += packet.len();
        self.packet_count += 1;
        true
    }

    pub fn packet_count(&self) -> usize {
        self.packet_count
    }

    pub fn is_empty(&self) -> bool {
        self.packet_count == 0
    }

    pub fn frame(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameDecodeError {
    Empty,
    UnknownVersion(u8),
}

pub fn decode_frame(frame: &[u8]) -> Result<EspNowFrameReader<'_>, FrameDecodeError> {
    let (&version, rest) = frame.split_first().ok_or(FrameDecodeError::Empty)?;
    if version != ESP_NOW_FRAME_VERSION {
        return Err(FrameDecodeError::UnknownVersion(version));
    }
    Ok(EspNowFrameReader { rest })
}

pub struct EspNowFrameReader<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for EspNowFrameReader<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.len() < ESP_NOW_LENGTH_PREFIX_LEN {
            return None;
        }
        let (len_bytes, after_len) = self.rest.split_at(ESP_NOW_LENGTH_PREFIX_LEN);
        let packet_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
        if after_len.len() < packet_len {
            // Truncated record — stop rather than yield a partial packet.
            self.rest = &[];
            return None;
        }
        let (packet, tail) = after_len.split_at(packet_len);
        self.rest = tail;
        Some(packet)
    }
}

pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        mode: InterfaceMode::Full,
        medium: MediumKind::SharedHalfDuplex,
        state: ConnectionState::Connected,
        announce_rate_limit: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn encoded_len_of_packets(packets: &[std::vec::Vec<u8>]) -> usize {
        ESP_NOW_FRAME_HEADER_LEN
            + packets
                .iter()
                .map(|packet| ESP_NOW_LENGTH_PREFIX_LEN + packet.len())
                .sum::<usize>()
    }

    fn fitting_packet_prefix(
        packets: std::vec::Vec<std::vec::Vec<u8>>,
    ) -> std::vec::Vec<std::vec::Vec<u8>> {
        let mut kept = std::vec::Vec::new();
        let mut used = ESP_NOW_FRAME_HEADER_LEN;

        for packet in packets {
            let need = ESP_NOW_LENGTH_PREFIX_LEN + packet.len();
            if used + need > ESP_NOW_MAX_FRAME_PAYLOAD {
                break;
            }
            used += need;
            kept.push(packet);
        }

        kept
    }

    fn fitting_packet_lists() -> impl Strategy<Value = std::vec::Vec<std::vec::Vec<u8>>> {
        prop::collection::vec(
            prop::collection::vec(any::<u8>(), 0..=crate::wire::MTU),
            0..12,
        )
        .prop_map(fitting_packet_prefix)
    }

    #[test]
    fn single_packet_round_trips_under_the_version_tag() {
        let packet = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x7E];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&packet));
        assert_eq!(w.packet_count(), 1);
        assert!(!w.is_empty());
        let frame = w.frame();
        assert_eq!(frame[0], ESP_NOW_FRAME_VERSION);

        let mut reader = decode_frame(frame).unwrap();
        assert_eq!(reader.next(), Some(&packet[..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn coalesces_several_packets_and_reads_them_back_in_order() {
        let a = [1u8, 2, 3];
        let b = [9u8; 250];
        let c = [0xAB, 0xCD];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&a));
        assert!(w.try_push(&b));
        assert!(w.try_push(&c));
        assert_eq!(w.packet_count(), 3);

        let mut reader = decode_frame(w.frame()).unwrap();
        assert_eq!(reader.next(), Some(&a[..]));
        assert_eq!(reader.next(), Some(&b[..]));
        assert_eq!(reader.next(), Some(&c[..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn the_fat_v2_frame_coalesces_at_least_two_full_mtu_packets() {
        let mtu_packet = [0x5Au8; crate::wire::MTU];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&mtu_packet));
        assert!(w.try_push(&mtu_packet));
        assert_eq!(w.packet_count(), 2);
        assert!(w.frame().len() <= ESP_NOW_MAX_FRAME_PAYLOAD);
    }

    #[test]
    fn try_push_refuses_a_packet_that_does_not_fit_and_leaves_a_valid_frame() {
        let mut buf = [0u8; ESP_NOW_FRAME_HEADER_LEN + ESP_NOW_LENGTH_PREFIX_LEN + 1];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&[0x11]));
        assert!(!w.try_push(&[0x22]));
        assert_eq!(w.packet_count(), 1);
        let mut reader = decode_frame(w.frame()).unwrap();
        assert_eq!(reader.next(), Some(&[0x11][..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn decode_rejects_empty_and_unknown_version() {
        assert_eq!(decode_frame(&[]).err(), Some(FrameDecodeError::Empty));
        let bogus = [ESP_NOW_FRAME_VERSION.wrapping_add(7), 0x00, 0x01, 0xFF];
        assert_eq!(
            decode_frame(&bogus).err(),
            Some(FrameDecodeError::UnknownVersion(
                ESP_NOW_FRAME_VERSION.wrapping_add(7)
            )),
        );
    }

    #[test]
    fn a_truncated_trailing_record_ends_iteration_after_the_clean_packets() {
        let frame = [
            ESP_NOW_FRAME_VERSION,
            0x00,
            0x02,
            0xAA,
            0xBB,
            0x00,
            0x09,
            0xCC,
        ];
        let mut reader = decode_frame(&frame).unwrap();
        assert_eq!(reader.next(), Some(&[0xAA, 0xBB][..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(InterfaceId::new([0xE9; 16]));
        assert!(matches!(d.medium, MediumKind::SharedHalfDuplex));
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert!(matches!(d.state, ConnectionState::Connected));
        assert_eq!(d.capabilities.ingress, IngressCapability::Enabled);
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
    }

    proptest! {
        #[test]
        fn arbitrary_fitting_packet_lists_round_trip_in_order(
            packets in fitting_packet_lists(),
        ) {
            let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
            let mut writer = EspNowFrameWriter::new(&mut buf);

            for packet in &packets {
                prop_assert!(writer.try_push(packet));
            }

            prop_assert_eq!(writer.packet_count(), packets.len());
            prop_assert_eq!(writer.is_empty(), packets.is_empty());
            prop_assert_eq!(writer.frame().len(), encoded_len_of_packets(&packets));

            let decoded: std::vec::Vec<std::vec::Vec<u8>> = decode_frame(writer.frame())
                .unwrap()
                .map(|packet| packet.to_vec())
                .collect();
            prop_assert_eq!(decoded, packets);
        }

        #[test]
        fn a_failed_push_leaves_the_existing_frame_valid_and_unchanged(
            packets in fitting_packet_lists(),
            fill_byte in any::<u8>(),
        ) {
            let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
            let mut writer = EspNowFrameWriter::new(&mut buf);

            for packet in &packets {
                prop_assert!(writer.try_push(packet));
            }

            let remaining = ESP_NOW_MAX_FRAME_PAYLOAD - writer.frame().len();
            let failing_packet = std::vec![fill_byte; remaining.saturating_sub(1)];
            let before_frame = writer.frame().to_vec();
            let before_count = writer.packet_count();

            prop_assert!(!writer.try_push(&failing_packet));
            prop_assert_eq!(writer.packet_count(), before_count);
            prop_assert_eq!(writer.frame(), before_frame.as_slice());

            let decoded: std::vec::Vec<std::vec::Vec<u8>> = decode_frame(writer.frame())
                .unwrap()
                .map(|packet| packet.to_vec())
                .collect();
            prop_assert_eq!(decoded, packets);
        }

        #[test]
        fn truncated_trailing_records_stop_after_the_last_clean_packet(
            packets in fitting_packet_lists(),
            truncated_tail in prop::collection::vec(any::<u8>(), 1..=crate::wire::MTU),
            visible_tail_len in 0usize..=crate::wire::MTU,
        ) {
            let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
            let mut writer = EspNowFrameWriter::new(&mut buf);
            for packet in &packets {
                prop_assert!(writer.try_push(packet));
            }

            let mut truncated = writer.frame().to_vec();
            truncated.extend_from_slice(&(truncated_tail.len() as u16).to_be_bytes());
            let partial_len = visible_tail_len.min(truncated_tail.len() - 1);
            truncated.extend_from_slice(&truncated_tail[..partial_len]);

            let decoded: std::vec::Vec<std::vec::Vec<u8>> = decode_frame(&truncated)
                .unwrap()
                .map(|packet| packet.to_vec())
                .collect();
            prop_assert_eq!(decoded, packets);
        }
    }
}
